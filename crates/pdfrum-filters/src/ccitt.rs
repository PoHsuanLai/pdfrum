//! `/CCITTFaxDecode` (ISO 32000-1 §7.4.6): Group 3 and Group 4 fax
//! compression, decoded through `hayro-ccitt`.
//!
//! This filter is reached from the image path rather than from
//! [`decode`](crate::decode), because unlike every other filter here it needs
//! the image dictionary: `/Columns` and `/Rows` default to *the image's* width
//! and height when they are zero, so the same stream decodes to different
//! shapes in different dictionaries.
//!
//! # Damaged rows come back white
//!
//! Every row is filled with white before decoding starts, and a row that
//! cannot be decoded is simply left that way. There is no error channel: a
//! stream that dies halfway through produces the rows it managed plus white to
//! the bottom, which is why a scanned page with a corrupt tail still shows its
//! top half. `/DamagedRowsBeforeError` — the parameter that exists precisely
//! to make this fail — is never read.
//!
//! # Bit convention
//!
//! Output is one bit per pixel, rows padded to a **four-byte** boundary (not
//! one byte, as every other filter here uses), with a set bit meaning *white*.
//! That is the inverse of `/ImageMask`'s convention and the inverse of what
//! `/BlackIs1 true` asks for — which is why `/BlackIs1` is implemented by
//! inverting, and why the image path must know which way round these bits are.

use hayro_ccitt::{DecodeSettings, Decoder, DecoderContext, EncodingMode};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Resolve, names};

use crate::Error;

/// The largest CCITT image either dimension may name.
const MAX_DIMENSION: i64 = 65535;

/// A decoded fax image: one bit per pixel, rows padded to four bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcittImage {
    /// Pixels across.
    pub width: u32,
    /// Rows.
    pub height: u32,
    /// Bytes per row, rounded up to a multiple of four.
    pub row_bytes: usize,
    /// `height * row_bytes` bytes, a set bit meaning white unless `/BlackIs1`
    /// asked for the opposite.
    pub bits: Vec<u8>,
}

/// `/DecodeParms` for `/CCITTFaxDecode`, with PDFium's defaults applied.
///
/// Every field is defaulted before the dictionary is consulted, so a missing
/// `/DecodeParms` and an empty one mean the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CcittParams {
    /// `/K`: negative for pure Group 4, zero for pure Group 3 one-dimensional,
    /// positive for mixed with a mode bit per row. Default 0.
    pub k: i64,
    /// `/EndOfLine`: whether rows are separated by end-of-line codes.
    pub end_of_line: bool,
    /// `/EncodedByteAlign`: whether each row starts on a byte boundary.
    pub encoded_byte_align: bool,
    /// `/BlackIs1`: whether a set bit means black. Default false, so a set bit
    /// means white.
    pub black_is_1: bool,
    /// `/Columns`: pixels per row, **1728** by default; zero means "use the
    /// image's own width".
    pub columns: i64,
    /// `/Rows`: rows in the image, zero by default and zero again if the file
    /// says more than 65535; zero means "use the image's own height".
    pub rows: i64,
}

impl Default for CcittParams {
    fn default() -> Self {
        Self {
            k: 0,
            end_of_line: false,
            encoded_byte_align: false,
            black_is_1: false,
            // The historic fax scanline width, and the specification's
            // default: a stream that omits /Columns is 1728 pixels wide even
            // when its image is not.
            columns: 1728,
            rows: 0,
        }
    }
}

impl CcittParams {
    /// Read a `/CCITTFaxDecode` parameter dictionary.
    ///
    /// ```
    /// use pdfrum_filters::CcittParams;
    /// use pdfrum_object::{Dict, NoResolve, Object, names};
    ///
    /// // An absent dictionary and an empty one agree on everything.
    /// assert_eq!(
    ///     CcittParams::from_dict(&Dict::new(), &NoResolve),
    ///     CcittParams::default()
    /// );
    ///
    /// // More rows than a fax can have resets the count, deferring to the
    /// // image's own /Height.
    /// let parms = Dict::from_pairs([(names::ROWS.clone(), Object::Int(70_000))]);
    /// assert_eq!(CcittParams::from_dict(&parms, &NoResolve).rows, 0);
    /// ```
    #[must_use]
    pub fn from_dict(d: &Dict, r: &impl Resolve) -> Self {
        let defaults = Self::default();
        let flag = |key| d.int(key, r).unwrap_or(0) != 0;
        let rows = d.int(names::ROWS, r).unwrap_or(defaults.rows);
        Self {
            k: d.int(names::K, r).unwrap_or(defaults.k),
            end_of_line: flag(names::END_OF_LINE),
            encoded_byte_align: flag(names::ENCODED_BYTE_ALIGN),
            black_is_1: flag(names::BLACK_IS_1),
            columns: d.int(names::COLUMNS, r).unwrap_or(defaults.columns),
            // A row count no fax can hold is not an error; it is ignored.
            rows: if rows > MAX_DIMENSION { 0 } else { rows },
        }
    }

    /// Resolve `/Columns` and `/Rows` against the image's own dimensions.
    ///
    /// Zero means "defer to the image"; anything else overrides it.
    ///
    /// # Errors
    ///
    /// [`Error::BadCcittParams`] when the resolved size is not positive or
    /// names more than 65535 pixels in either direction. A negative
    /// `/Columns` reaches here — the parameter reader accepts it — and is
    /// rejected at this point, as in the C++.
    pub fn resolve_size(self, image_width: u32, image_height: u32) -> Result<(u32, u32), Error> {
        let width = if self.columns != 0 {
            self.columns
        } else {
            i64::from(image_width)
        };
        let height = if self.rows != 0 {
            self.rows
        } else {
            i64::from(image_height)
        };
        if width <= 0 || height <= 0 {
            return Err(Error::BadCcittParams("the image has no pixels"));
        }
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(Error::BadCcittParams(
                "the image is larger than 65535 pixels",
            ));
        }
        // Both are in 1..=65535 here, so neither conversion can fail.
        let fit = |n: i64| u32::try_from(n).map_err(|_| Error::SizeOverflow);
        Ok((fit(width)?, fit(height)?))
    }
}

/// Decode a `/CCITTFaxDecode` stream against an image `image_width` by
/// `image_height` pixels.
///
/// The dimensions are only consulted where `params` defers to them. Decoding
/// never fails on damaged data: rows that cannot be read stay white and a
/// diagnostic is recorded.
///
/// # Errors
///
/// [`Error::BadCcittParams`] when the parameters and the image dimensions
/// between them describe an image no decoder can produce,
/// [`Error::SizeOverflow`] when the row stride and height do not multiply,
/// and [`Error::OutputTooLarge`] when the resulting buffer would pass
/// `limits.max_decoded_stream_len`. The dimensions are attacker-controlled up
/// to 65535 in each direction, so the size is settled before anything is
/// allocated.
pub fn decode_ccitt(
    input: &[u8],
    params: CcittParams,
    image_width: u32,
    image_height: u32,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<CcittImage, Error> {
    let (width, height) = params.resolve_size(image_width, image_height)?;

    // Rows are padded to a whole number of 32-bit words, unlike every other
    // filter's byte-aligned rows.
    let row_bytes = usize::try_from(width)
        .map_err(|_| Error::SizeOverflow)?
        .div_ceil(32)
        * 4;
    let total = row_bytes
        .checked_mul(usize::try_from(height).map_err(|_| Error::SizeOverflow)?)
        .ok_or(Error::SizeOverflow)?;
    if total > limits.max_decoded_stream_len {
        return Err(Error::OutputTooLarge {
            limit: limits.max_decoded_stream_len,
        });
    }

    let encoding = match params.k.cmp(&0) {
        std::cmp::Ordering::Less => EncodingMode::Group4,
        std::cmp::Ordering::Equal => EncodingMode::Group3_1D,
        std::cmp::Ordering::Greater => EncodingMode::Group3_2D {
            k: u32::try_from(params.k).unwrap_or(u32::MAX),
        },
    };
    let settings = DecodeSettings {
        columns: width,
        rows: height,
        end_of_block: true,
        end_of_line: params.end_of_line,
        rows_are_byte_aligned: params.encoded_byte_align,
        encoding,
        invert_black: false,
    };

    // Every bit starts white; a row nothing writes to stays that way.
    let mut sink = RowSink {
        bits: vec![0xff; total],
        row_bytes,
        row: 0,
        column: 0,
    };
    let mut context = DecoderContext::new(settings);
    let outcome = hayro_ccitt::decode(input, &mut sink, &mut context);

    let mut bits = sink.bits;
    if params.black_is_1 {
        // A set bit should mean black, so flip the whole buffer — padding
        // included, exactly as the C++'s word-at-a-time inversion does.
        for byte in &mut bits {
            *byte = !*byte;
        }
    }
    if outcome.is_err() || sink.row < height {
        diags.record(
            Severity::Recovered,
            DiagKind::UndecodableStream,
            Some(u64::from(sink.row)),
        );
    }
    Ok(CcittImage {
        width,
        height,
        row_bytes,
        bits,
    })
}

/// Collects the decoder's pixels into a padded bitmap, white being a set bit.
struct RowSink {
    bits: Vec<u8>,
    row_bytes: usize,
    row: u32,
    column: u32,
}

impl RowSink {
    /// Where in `bits` the pixel at the current position lives.
    fn offset(&self) -> Option<usize> {
        usize::try_from(self.row)
            .ok()?
            .checked_mul(self.row_bytes)?
            .checked_add(usize::try_from(self.column).ok()? / 8)
    }
}

impl Decoder for RowSink {
    fn push_pixel(&mut self, white: bool) {
        if let Some(byte) = self.offset().and_then(|at| self.bits.get_mut(at)) {
            let mask = 0x80u8 >> (self.column % 8);
            if white {
                *byte |= mask;
            } else {
                *byte &= !mask;
            }
        }
        self.column = self.column.saturating_add(1);
    }

    fn push_pixel_chunk(&mut self, white: bool, chunk_count: u32) {
        // Only ever called on a byte boundary, so whole bytes can be written.
        let fill = if white { 0xff } else { 0x00 };
        for _ in 0..chunk_count {
            if let Some(byte) = self.offset().and_then(|at| self.bits.get_mut(at)) {
                *byte = fill;
            }
            self.column = self.column.saturating_add(8);
        }
    }

    fn next_line(&mut self) {
        self.row = self.row.saturating_add(1);
        self.column = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{CcittParams, decode_ccitt};
    use crate::Error;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object, names};

    fn parms(pairs: &[(&Name, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(key, value)| ((*key).clone(), value.clone())),
        )
    }

    fn read(pairs: &[(&Name, Object)]) -> CcittParams {
        CcittParams::from_dict(&parms(pairs), &NoResolve)
    }

    #[test]
    fn an_absent_and_an_empty_dictionary_agree() {
        let defaults = CcittParams::default();
        assert_eq!(read(&[]), defaults);
        assert_eq!(defaults.k, 0);
        assert_eq!(defaults.columns, 1728, "the historic fax width");
        assert_eq!(defaults.rows, 0);
        assert!(!defaults.end_of_line);
        assert!(!defaults.encoded_byte_align);
        assert!(!defaults.black_is_1);
    }

    #[test]
    fn the_default_width_ignores_the_image() {
        // 1728 is a real default, not "whatever the image says".
        assert_eq!(read(&[]).resolve_size(100, 50).expect("valid"), (1728, 50));
    }

    #[test]
    fn columns_and_rows_override_the_image_when_nonzero() {
        let params = read(&[
            (names::COLUMNS, Object::Int(100)),
            (names::ROWS, Object::Int(20)),
        ]);
        assert_eq!(params.resolve_size(640, 480).expect("valid"), (100, 20));
    }

    #[test]
    fn zero_defers_to_the_image() {
        let params = read(&[
            (names::COLUMNS, Object::Int(0)),
            (names::ROWS, Object::Int(0)),
        ]);
        assert_eq!(params.resolve_size(100, 50).expect("valid"), (100, 50));
    }

    #[test]
    fn a_negative_column_count_passes_the_reader_and_fails_the_resolver() {
        let params = read(&[(names::COLUMNS, Object::Int(-1))]);
        assert_eq!(params.columns, -1, "the reader takes it as written");
        assert!(matches!(
            params.resolve_size(100, 50),
            Err(Error::BadCcittParams(_))
        ));
    }

    #[test]
    fn an_oversized_column_count_is_rejected() {
        let params = read(&[(names::COLUMNS, Object::Int(70_000))]);
        assert!(matches!(
            params.resolve_size(100, 50),
            Err(Error::BadCcittParams(_))
        ));
    }

    // /Rows is clamped by resetting it, not by rejecting it — so an absurd
    // row count silently defers to the image's own height.
    #[test]
    fn an_oversized_row_count_resets_to_the_image_height() {
        let params = read(&[(names::ROWS, Object::Int(70_000))]);
        assert_eq!(params.rows, 0);
        assert_eq!(params.resolve_size(100, 50).expect("valid").1, 50);
    }

    #[test]
    fn the_flags_read_as_integers_not_booleans() {
        let params = read(&[
            (names::K, Object::Int(-1)),
            (names::BLACK_IS_1, Object::Bool(true)),
            (names::ENCODED_BYTE_ALIGN, Object::Int(1)),
            (names::END_OF_LINE, Object::Int(0)),
        ]);
        assert_eq!(params.k, -1);
        // A boolean has an integer reading of 1, so this works either way.
        assert!(params.black_is_1);
        assert!(params.encoded_byte_align);
        assert!(!params.end_of_line);
    }

    #[test]
    fn rows_are_padded_to_four_bytes() {
        let mut diags = Diagnostics::default();
        for (width, expected) in [(1u32, 4usize), (8, 4), (32, 4), (33, 8), (100, 16)] {
            let params = CcittParams {
                columns: i64::from(width),
                rows: 2,
                ..CcittParams::default()
            };
            let image = decode_ccitt(b"", params, 0, 0, &Limits::default(), &mut diags)
                .expect("valid size");
            assert_eq!(image.row_bytes, expected, "width {width}");
            assert_eq!(image.bits.len(), expected * 2);
        }
    }

    // The behavior the whole wrapper exists to guarantee: whatever the input,
    // the caller gets a correctly shaped image and never an error, and rows
    // nothing wrote to are white.
    #[test]
    fn an_undecodable_stream_comes_back_white_rather_than_failing() {
        let params = CcittParams {
            k: -1,
            columns: 64,
            rows: 4,
            ..CcittParams::default()
        };
        let mut diags = Diagnostics::default();
        for input in [&b""[..], b"\xde\xad\xbe\xef", b"\x00\x00\x00", b"\xff"] {
            let image = decode_ccitt(input, params, 64, 4, &Limits::default(), &mut diags)
                .expect("damage is never an error");
            assert_eq!(image.width, 64);
            assert_eq!(image.height, 4);
            assert_eq!(image.bits.len(), image.row_bytes * 4, "fully sized");
        }
        // An empty stream cannot produce a single row, so every bit stays as
        // the white prefill left it.
        let image =
            decode_ccitt(b"", params, 64, 4, &Limits::default(), &mut diags).expect("valid");
        assert!(image.bits.iter().all(|&b| b == 0xff), "all white");
    }

    #[test]
    fn a_stream_that_stops_early_keeps_its_decoded_rows_and_whitens_the_rest() {
        // One Group 4 row's worth of codes, in an image claiming eight rows.
        let params = CcittParams {
            k: -1,
            columns: 8,
            rows: 8,
            ..CcittParams::default()
        };
        let mut diags = Diagnostics::default();
        let image = decode_ccitt(&[0b1000_0000], params, 8, 8, &Limits::default(), &mut diags)
            .expect("truncation is never an error");
        assert_eq!(image.height, 8);
        assert_eq!(image.bits.len(), image.row_bytes * 8);
        assert!(
            image
                .bits
                .get(image.row_bytes..)
                .is_some_and(|tail| tail.iter().all(|&b| b == 0xff)),
            "the rows nothing reached are white"
        );
    }

    #[test]
    fn black_is_one_inverts_the_whole_buffer() {
        let params = CcittParams {
            k: -1,
            columns: 64,
            rows: 2,
            black_is_1: true,
            ..CcittParams::default()
        };
        let mut diags = Diagnostics::default();
        let image =
            decode_ccitt(b"", params, 64, 2, &Limits::default(), &mut diags).expect("valid");
        assert!(
            image.bits.iter().all(|&b| b == 0x00),
            "white prefill inverted"
        );
    }

    #[test]
    fn a_real_group_four_stream_decodes_its_pixels() {
        // A 8x1 all-white row in Group 4: a vertical-zero code (1) puts the
        // first changing element at the row's end.
        let params = CcittParams {
            k: -1,
            columns: 8,
            rows: 1,
            ..CcittParams::default()
        };
        let mut diags = Diagnostics::default();
        let image = decode_ccitt(&[0b1000_0000], params, 8, 1, &Limits::default(), &mut diags)
            .expect("valid");
        assert_eq!(image.bits.first(), Some(&0xff), "an all-white row");
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        let mut diags = Diagnostics::default();
        for k in [-1i64, 0, 4] {
            for seed in 0..24u8 {
                let junk: Vec<u8> = (0..128u8)
                    .map(|i| i.wrapping_mul(seed).wrapping_add(7))
                    .collect();
                let params = CcittParams {
                    k,
                    columns: 37,
                    rows: 9,
                    encoded_byte_align: seed % 2 == 0,
                    ..CcittParams::default()
                };
                let _ = decode_ccitt(&junk, params, 37, 9, &Limits::default(), &mut diags);
            }
        }
    }

    // `/Columns` and `/Rows` are the stream's to declare, and the two together
    // name the buffer. An empty stream claiming the largest image the
    // parameters allow asks for half a gigabyte; the budget answers before the
    // allocation happens.
    #[test]
    fn a_vast_declared_image_is_refused_rather_than_allocated() {
        let params = CcittParams {
            columns: 65535,
            rows: 65535,
            ..CcittParams::default()
        };
        let limits = Limits {
            max_decoded_stream_len: 1024 * 1024,
            ..Limits::default()
        };
        let mut diags = Diagnostics::default();
        assert_eq!(
            decode_ccitt(b"", params, 0, 0, &limits, &mut diags).err(),
            Some(Error::OutputTooLarge { limit: 1024 * 1024 })
        );
    }

    // The same image under a budget that accommodates it still decodes, so the
    // check bounds the buffer without narrowing what the decoder accepts.
    #[test]
    fn an_image_within_the_budget_still_decodes() {
        let params = CcittParams {
            columns: 64,
            rows: 4,
            ..CcittParams::default()
        };
        let limits = Limits {
            max_decoded_stream_len: 32,
            ..Limits::default()
        };
        let mut diags = Diagnostics::default();
        let image = decode_ccitt(b"", params, 0, 0, &limits, &mut diags).expect("32 bytes fit");
        assert_eq!(image.bits.len(), 32);
    }
}
