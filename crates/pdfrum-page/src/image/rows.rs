//! Image samples as a pull pipeline of rows.
//!
//! An image reaches the device as a sequence of *stages*, and a stage is a
//! type. Each one yields the image one row at a time, at source width, into a
//! buffer it owns and reuses; the next stage borrows that row and produces its
//! own. The representation of a row is its type, so a stage can only accept
//! what the previous one produces — and a full-size intermediate copy cannot
//! exist, because no type in the chain asks for one.
//!
//! This is the shape PDFium's image path has had all along.
//! `CStretchEngine::Continue` pulls `CPDF_DIB::GetScanline` per source row
//! (`cstretchengine.cpp:346`), so unpacking, colour conversion and the
//! horizontal stretch all happen inside one scanline's lifetime and the only
//! full-height buffer is destination-width.
//!
//! # The stages
//!
//! ```text
//! Pixels ──[Source]──> Row<'_, P>  ──[Converted]──> Row<'_, Rgba8>
//! ```
//!
//! [`Converted`] is the one place a sample becomes a colour, and
//! `convert_row` is the one function that does it. There is no
//! per-pixel entry point beside it: a caller that wants a whole image walks
//! the rows, and a caller that wants one pixel does not exist.
//!
//! # Where the packed samples come in
//!
//! [`Source`] takes [`Samples`], not [`Pixels`]: for a [`Samples::Packed`]
//! image it drives an [`Unpacked`] stage that widens the row it is about to
//! yield, and for a [`Samples::Whole`] one it borrows straight out of the
//! decoded buffer. So the pipeline is `Unpacked -> Converted` for everything
//! the filter chain left packed: there is no full-size widened buffer.

use crate::color::{Rgb, adobe_cmyk_to_srgb};
use crate::image::{BitImage, Pixels, Samples, Unpacked};

/// A run of pixels in one representation.
///
/// The lifetime is the stage's own buffer: a row is borrowed for as long as
/// the caller is looking at it and is overwritten by the next `next` call,
/// which is what keeps the pipeline to one row of working memory per stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row<'a, P>(&'a [P]);

impl<'a, P> Row<'a, P> {
    /// The pixels.
    #[must_use]
    pub const fn pixels(self) -> &'a [P] {
        self.0
    }
}

/// Whatever yields an image's samples one row at a time, at source width.
///
/// The one seam of the pipeline. `next` returns `None` at the end of the
/// image and never afterwards yields anything again; a stage that cannot
/// produce a row it expected yields the fallback its own documentation names
/// rather than stopping early, because a malformed image must still paint.
pub trait Rows {
    /// What one pixel of a row this stage produces looks like.
    type Pixel;

    /// The next row, or `None` when the image is exhausted.
    fn next(&mut self) -> Option<Row<'_, Self::Pixel>>;
}

/// Premultiplied RGBA, the representation the device buffer wants.
///
/// `#[repr(C)]` over an array so a row of them is a row of pixels to index,
/// not a row of bytes to multiply an index by four.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct Rgba8(pub [u8; 4]);

/// Eight-bit red, green, blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct Rgb8(pub [u8; 3]);

/// A palette with every entry already in the representation a row wants.
///
/// A palette has at most 256 entries and an image has as many pixels as it
/// has, so resolving it once and indexing it is the same answer as resolving
/// per pixel for a fraction of the work. Built once when the pipeline is,
/// never per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette(Box<[Rgb8]>);

impl Palette {
    /// Encode a space's resolved palette.
    #[must_use]
    pub fn new(entries: &[Rgb]) -> Self {
        Self(entries.iter().map(|c| Rgb8(c.to_bytes())).collect())
    }

    /// The entry for an index, or black when the palette does not have one —
    /// which is `color_at`'s own fallback, kept deliberately.
    #[must_use]
    pub fn get(&self, index: u8) -> Rgb8 {
        self.0
            .get(usize::from(index))
            .copied()
            .unwrap_or(Rgb8([0, 0, 0]))
    }
}

/// The source stage: an image's [`Samples`] walked one row at a time.
///
/// A [`Samples::Whole`] arm borrows straight out of the decoded buffer, so it
/// owns no memory at all — except for [`Pixels::Stencil`], whose bits have to
/// be widened into something a row can borrow. A [`Samples::Packed`] one owns
/// the [`Unpacked`] stage's single row buffer, and nothing more.
#[derive(Debug)]
pub struct Source<'a> {
    kind: SourceKind<'a>,
    width: usize,
    height: u32,
    y: u32,
}

/// Which representation a [`Source`] is walking.
#[derive(Debug)]
enum SourceKind<'a> {
    /// A stencil widened one row at a time into `scratch`.
    Stencil {
        bits: &'a BitImage,
        scratch: Vec<u8>,
    },
    /// Still-packed samples, widened one row at a time by [`Unpacked`].
    ///
    /// The component count decides which [`Samples`] arm the widened row is,
    /// exactly as it decided which [`Pixels`] variant the eager pass built.
    Packed {
        rows: Unpacked<'a>,
        components: usize,
    },
    Gray(&'a [u8]),
    Rgb(&'a [u8]),
    Cmyk(&'a [u8]),
    Indexed(&'a [u8]),
}

/// One row of a [`Source`], in whichever representation the image has.
///
/// An enum rather than a generic parameter because the *image* decides which
/// arm it is, at run time, and a caller that must handle all five would
/// otherwise be five monomorphised copies of the same loop.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SampleRow<'a> {
    /// One grey component per pixel. A stencil arrives here too, already
    /// widened: a set bit is ink, which is 0, and a clear one is 255.
    Gray(&'a [u8]),
    /// Three components per pixel, red then green then blue.
    Rgb(&'a [u8]),
    /// Four components per pixel, cyan, magenta, yellow, black.
    Cmyk(&'a [u8]),
    /// One palette index per pixel.
    Indexed(&'a [u8]),
}

impl<'a> Source<'a> {
    /// Start walking `samples`, an image `width` by `height`.
    #[must_use]
    pub fn new(samples: &'a Samples, width: u32, height: u32) -> Self {
        let width = width as usize;
        let kind = match samples {
            Samples::Packed(p) => SourceKind::Packed {
                rows: Unpacked::new(p),
                components: p.components(),
            },
            Samples::Whole(Pixels::Stencil(bits)) => SourceKind::Stencil {
                bits,
                scratch: vec![0_u8; width],
            },
            Samples::Whole(Pixels::Gray8(d)) => SourceKind::Gray(d),
            Samples::Whole(Pixels::Rgb8(d)) => SourceKind::Rgb(d),
            Samples::Whole(Pixels::Cmyk8(d)) => SourceKind::Cmyk(d),
            Samples::Whole(Pixels::Indexed { indices, .. }) => SourceKind::Indexed(indices),
        };
        Self {
            kind,
            width,
            height,
            y: 0,
        }
    }

    /// A source positioned to yield exactly the one row `y`, and then stop.
    ///
    /// Tests reach for a single pixel — including one far past the end, to
    /// check the fallback — and walking down to its row from the top would be
    /// quadratic in `y`. The pipeline itself never wants this: the render path
    /// walks every row in order, which is the whole point of it.
    #[cfg(test)]
    pub(crate) fn at_row(samples: &'a Samples, width: u32, y: u32) -> Self {
        let mut source = Self::new(samples, width, y.saturating_add(1));
        match &mut source.kind {
            // A packed source has no index to skip to — its rows come out of
            // a bit walk that has to run — so it is pulled forward instead.
            // Only tests reach this, and only for small `y`.
            SourceKind::Packed { rows, .. } => {
                for _ in 0..y {
                    if rows.next_row().is_none() {
                        break;
                    }
                }
                source.y = y;
            }
            _ => source.y = y,
        }
        source
    }

    /// The next row of samples, or `None` past the bottom of the image.
    ///
    /// A row the buffer only partly holds yields **the samples it has**, not
    /// nothing: the per-pixel path this replaced read every sample with a
    /// `get(..).unwrap_or(0)`, so a truncated stream painted the bytes that
    /// were there and black beyond them. Dropping the row instead would move
    /// that boundary, which `a_truncated_stream_is_zero_padded_rather_than_
    /// rejected` and the corpus's damaged images both pin. [`Converted`]
    /// clears the tail of its own buffer so the missing pixels read as black
    /// rather than as the row before.
    pub(crate) fn next_row(&mut self) -> Option<SampleRow<'_>> {
        if self.y >= self.height {
            return None;
        }
        let y = self.y as usize;
        self.y += 1;
        let width = self.width;
        // The row's byte range for a `components`-wide representation,
        // clipped to what the buffer actually holds and trimmed to a whole
        // number of pixels.
        let span = |components: usize, len: usize| -> Option<(usize, usize)> {
            let stride = width.checked_mul(components)?;
            let at = y.checked_mul(stride)?;
            // `at` can be past the end entirely, for a row the buffer never
            // reached: that is a zero-length span, not an absent row.
            let end = at.checked_add(stride)?.min(len).max(at);
            let whole = (end - at) / components * components;
            Some((at.min(len), at.min(len).checked_add(whole)?))
        };
        match &mut self.kind {
            SourceKind::Stencil { bits, scratch } => {
                for (x, slot) in scratch.iter_mut().enumerate() {
                    let set = u32::try_from(x).is_ok_and(|x| bits.pixel(x, self.y - 1));
                    // A set bit is ink, which reads 0; a clear one is paper,
                    // which reads 255. The stencil's *colour* is applied
                    // later, so this stage only preserves that convention.
                    *slot = if set { 0 } else { 255 };
                }
                Some(SampleRow::Gray(scratch))
            }
            SourceKind::Gray(d) => {
                let (a, b) = span(1, d.len())?;
                Some(SampleRow::Gray(d.get(a..b)?))
            }
            SourceKind::Rgb(d) => {
                let (a, b) = span(3, d.len())?;
                Some(SampleRow::Rgb(d.get(a..b)?))
            }
            SourceKind::Cmyk(d) => {
                let (a, b) = span(4, d.len())?;
                Some(SampleRow::Cmyk(d.get(a..b)?))
            }
            SourceKind::Indexed(d) => {
                let (a, b) = span(1, d.len())?;
                Some(SampleRow::Indexed(d.get(a..b)?))
            }
            // `Unpacked` yields exactly the row the eager pass would have
            // written, already at a byte per component, so the representation
            // is the component count and nothing else.
            SourceKind::Packed { rows, components } => {
                let row = rows.next_row()?;
                Some(match *components {
                    1 => SampleRow::Gray(row),
                    4 => SampleRow::Cmyk(row),
                    _ => SampleRow::Rgb(row),
                })
            }
        }
    }
}

/// Components to premultiplied RGBA, one row at a time.
///
/// The second and last stage of the pipeline: it takes whatever
/// [`Source`] produced, runs it through `convert_row`, and joins
/// the mask alpha, the matte and the transfer function — all per-pixel
/// decisions that belong to this stage.
#[derive(Debug)]
pub struct Converted<'a> {
    source: Source<'a>,
    palette: Option<Palette>,
    buf: Vec<Rgba8>,
}

impl<'a> Converted<'a> {
    /// Convert the rows of `source`, resolving indices through `palette`.
    ///
    /// The palette is `Some` exactly when the source is [`Pixels::Indexed`];
    /// an indexed row with no palette resolves every index to black, which is
    /// the fallback the per-pixel path had.
    #[must_use]
    pub fn new(source: Source<'a>, palette: Option<Palette>) -> Self {
        let width = source.width;
        Self {
            source,
            palette,
            buf: vec![Rgba8::default(); width],
        }
    }

    /// The next row converted in place, handed to `finish` for the alpha, the
    /// matte and the transfer function before anyone else sees it.
    ///
    /// The conversion itself produces *opaque* RGBA — alpha is not the colour
    /// space's business — and `finish` is where the caller's own per-pixel
    /// business goes. Passing it in rather than exposing the buffer keeps the
    /// row's mutable lifetime inside this stage, which is what lets the next
    /// stage borrow the finished row immutably straight afterwards.
    pub fn next_row_with(&mut self, finish: impl FnOnce(&mut [Rgba8])) -> Option<Row<'_, Rgba8>> {
        let samples = self.source.next_row()?;
        convert_row(samples, self.palette.as_ref(), &mut self.buf);
        finish(&mut self.buf);
        Some(Row(&self.buf))
    }
}

impl Rows for Converted<'_> {
    type Pixel = Rgba8;

    fn next(&mut self) -> Option<Row<'_, Rgba8>> {
        self.next_row_with(|_| ())
    }
}

/// The one conversion: a row of samples becomes a row of opaque RGBA.
///
/// Every arm is the arithmetic `Pixels::sample_bytes` ran per pixel, hoisted
/// to a row: the same table for CMYK, the same palette fallback for indices,
/// the same widening for grey. What changed is that the match on the image's
/// representation happens once per row instead of once per pixel, and the
/// index arithmetic is a walk rather than a multiply and two bounds checks.
///
/// A `dst` shorter than the row converts as much as it holds. A `dst` longer
/// than it — which is a row the source could only partly supply — takes
/// **opaque black** in the tail, because that is what the per-pixel path
/// produced: it read each missing component through `get(..).unwrap_or(0)`
/// and then made a colour of the zeroes.
fn convert_row(samples: SampleRow<'_>, palette: Option<&Palette>, dst: &mut [Rgba8]) {
    let converted = match samples {
        SampleRow::Gray(src) => {
            for (slot, &v) in dst.iter_mut().zip(src) {
                *slot = Rgba8([v, v, v, 255]);
            }
            src.len()
        }
        SampleRow::Rgb(src) => {
            for (slot, px) in dst.iter_mut().zip(src.chunks_exact(3)) {
                let rgb = [px.first(), px.get(1), px.get(2)].map(|c| c.copied().unwrap_or(0));
                *slot = Rgba8([rgb[0], rgb[1], rgb[2], 255]);
            }
            src.len() / 3
        }
        SampleRow::Cmyk(src) => {
            for (slot, px) in dst.iter_mut().zip(src.chunks_exact(4)) {
                let cmyk =
                    [px.first(), px.get(1), px.get(2), px.get(3)].map(|v| v.copied().unwrap_or(0));
                let [cyan, magenta, yellow, black] = cmyk;
                let rgb = adobe_cmyk_to_srgb(cyan, magenta, yellow, black);
                *slot = Rgba8([rgb[0], rgb[1], rgb[2], 255]);
            }
            src.len() / 4
        }
        SampleRow::Indexed(src) => {
            for (slot, &index) in dst.iter_mut().zip(src) {
                let Rgb8(rgb) = palette.map_or(Rgb8([0, 0, 0]), |p| p.get(index));
                *slot = Rgba8([rgb[0], rgb[1], rgb[2], 255]);
            }
            src.len()
        }
    };
    // The tail the source could not supply. Without this it would show the
    // previous row, since the buffer is reused across rows.
    if let Some(tail) = dst.get_mut(converted..) {
        let black = match samples {
            // An absent *index* is 0, and index 0's palette entry is a real
            // colour — the same ladder the per-pixel path walked.
            SampleRow::Indexed(_) => {
                let Rgb8(rgb) = palette.map_or(Rgb8([0, 0, 0]), |p| p.get(0));
                Rgba8([rgb[0], rgb[1], rgb[2], 255])
            }
            SampleRow::Cmyk(_) => {
                let rgb = adobe_cmyk_to_srgb(0, 0, 0, 0);
                Rgba8([rgb[0], rgb[1], rgb[2], 255])
            }
            SampleRow::Gray(_) | SampleRow::Rgb(_) => Rgba8([0, 0, 0, 255]),
        };
        tail.fill(black);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(data: &[u8]) -> Samples {
        Samples::Whole(Pixels::Gray8(data.into()))
    }

    /// The still-packed form of the same grey samples: eight bits, one
    /// component, the identity decode.
    fn packed_gray(data: &[u8], width: u32, height: u32) -> Samples {
        Samples::Packed(crate::image::Packed::new(
            data.into(),
            crate::image::Depth::Eight,
            1,
            width as usize,
            width,
            height,
            &crate::color::ColorSpace::DeviceGray,
            None,
        ))
    }

    #[test]
    fn a_grey_row_widens_to_opaque_rgba() {
        let px = gray(&[0, 128, 255, 7]);
        let mut c = Converted::new(Source::new(&px, 2, 2), None);
        let first = c.next().expect("first row").pixels().to_vec();
        assert_eq!(
            first,
            vec![Rgba8([0, 0, 0, 255]), Rgba8([128, 128, 128, 255])]
        );
        let second = c.next().expect("second row").pixels().to_vec();
        assert_eq!(
            second,
            vec![Rgba8([255, 255, 255, 255]), Rgba8([7, 7, 7, 255])]
        );
        assert!(c.next().is_none());
    }

    #[test]
    fn an_rgb_row_keeps_its_component_order() {
        let px = Samples::Whole(Pixels::Rgb8(Box::new([1, 2, 3, 4, 5, 6])));
        let mut c = Converted::new(Source::new(&px, 2, 1), None);
        let row = c.next().expect("row").pixels().to_vec();
        assert_eq!(row, vec![Rgba8([1, 2, 3, 255]), Rgba8([4, 5, 6, 255])]);
    }

    #[test]
    fn an_indexed_row_reads_its_palette_and_falls_back_to_black() {
        let px = Samples::Whole(Pixels::Indexed {
            indices: Box::new([0, 1, 9]),
            palette: Box::new([]),
        });
        let palette = Palette::new(&[
            Rgb {
                r: 1.0,
                g: 0.0,
                b: 0.0,
            },
            Rgb {
                r: 0.0,
                g: 1.0,
                b: 0.0,
            },
        ]);
        let mut c = Converted::new(Source::new(&px, 3, 1), Some(palette));
        let row = c.next().expect("row").pixels().to_vec();
        assert_eq!(
            row,
            vec![
                Rgba8([255, 0, 0, 255]),
                Rgba8([0, 255, 0, 255]),
                // Index 9 is past the end of a two-entry palette: black.
                Rgba8([0, 0, 0, 255]),
            ]
        );
    }

    #[test]
    fn a_stencils_set_bit_is_ink_and_its_clear_bit_is_paper() {
        // One byte holds a row of up to eight bits, MSB first: `0b1010_0000`
        // is set, clear, set, clear across four pixels.
        let bits = BitImage {
            width: 4,
            height: 1,
            row_bytes: 1,
            bits: vec![0b1010_0000],
        };
        let px = Samples::Whole(Pixels::Stencil(bits));
        let mut c = Converted::new(Source::new(&px, 4, 1), None);
        let row = c.next().expect("row").pixels().to_vec();
        assert_eq!(
            row,
            vec![
                Rgba8([0, 0, 0, 255]),
                Rgba8([255, 255, 255, 255]),
                Rgba8([0, 0, 0, 255]),
                Rgba8([255, 255, 255, 255]),
            ]
        );
    }

    /// A buffer too short for the dimensions paints the samples it has and
    /// black beyond them, for every row the image declares.
    ///
    /// This is the per-pixel path's own degradation — every sample was read
    /// through `get(..).unwrap_or(0)` — and moving that boundary would change
    /// every damaged image in the corpus. A partial row keeps its samples; a
    /// wholly absent one is still yielded, all black.
    #[test]
    fn a_short_buffer_paints_black_past_its_end() {
        // Five bytes of a 2x4 grey image: rows 0 and 1 whole, row 2 half, row
        // 3 absent.
        let px = gray(&[1, 2, 3, 4, 5]);
        let mut c = Converted::new(Source::new(&px, 2, 4), None);
        let black = Rgba8([0, 0, 0, 255]);
        let row = |v: &[u8]| -> Vec<Rgba8> { v.iter().map(|&b| Rgba8([b, b, b, 255])).collect() };
        assert_eq!(c.next().expect("row 0").pixels(), row(&[1, 2]));
        assert_eq!(c.next().expect("row 1").pixels(), row(&[3, 4]));
        // The half row keeps its one sample; the missing one is black, not a
        // leftover of the row before.
        assert_eq!(
            c.next().expect("row 2").pixels(),
            vec![Rgba8([5, 5, 5, 255]), black]
        );
        assert_eq!(c.next().expect("row 3").pixels(), vec![black, black]);
        // And the image ends where it said it would.
        assert!(c.next().is_none());
    }

    /// The lazy arm and the eager one are the same rows.
    ///
    /// `Unpacked` widens as the pipeline pulls; the eager `unpack` widened
    /// first and `Source` walked the result. For eight-bit identity-decoded
    /// samples the two are the same bytes, which is what makes step 5 a move
    /// of work rather than a change of arithmetic.
    #[test]
    fn a_packed_source_yields_what_an_unpacked_one_does() {
        let data: Vec<u8> = (0..12u8).map(|i| i.wrapping_mul(23)).collect();
        let eager = gray(&data);
        let lazy = packed_gray(&data, 3, 4);
        let mut a = Converted::new(Source::new(&eager, 3, 4), None);
        let mut b = Converted::new(Source::new(&lazy, 3, 4), None);
        for _ in 0..4 {
            let want = a.next().expect("an eager row").pixels().to_vec();
            let got = b.next().expect("a lazy row").pixels().to_vec();
            assert_eq!(want, got);
        }
        assert!(a.next().is_none());
        assert!(b.next().is_none());
    }
}
