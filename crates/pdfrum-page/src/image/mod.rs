//! Image `XObject`s: from a stream to pixels (ISO 32000-1 §8.9).
//!
//! The load is a ladder, and every rung has a failure mode worth knowing:
//!
//! 1. **Validate the dictionary** — dimensions, bit depth, the two
//!    filter-driven coercions. There is no "repair a bad bit depth to eight";
//!    see [`dict`].
//! 2. **Resolve the colour space**, consulting form resources only for inline
//!    images.
//! 3. **Build the decode mapping** from `/Decode`, or the space's defaults.
//! 4. **Run the codec** the last filter names, or read raw samples.
//! 5. **Load the mask**, where `/SMask` beats `/Mask` and a mask that fails
//!    to load is simply dropped rather than failing the image.
//!
//! # Owned pixels, not a lazy scanline source
//!
//! PDFium produces scanlines on demand from three mutable scratch buffers.
//! We hold an owned [`ImageData`] instead: every
//! per-scanline *behaviour* is preserved — the truncated-stream zero pad, the
//! palette packing, the sixteen-bit high-byte truncation — and only the
//! laziness is gone. Memory is bounded by the same four-gibibyte cap the C++
//! enforces.

mod bitimage;
mod cache;
mod dct;
mod decode_array;
mod dict;
#[cfg(feature = "jbig2")]
mod jbig2;
#[cfg(feature = "jpeg2000")]
mod jpx;
mod mask;
mod packed;
mod rows;
mod scanline;

pub use bitimage::BitImage;
pub use cache::{ImageCache, MAX_BYTES, RequestedSize};
pub(crate) use dct::decode_dct;
pub(crate) use decode_array::DecodeMap;
pub(crate) use dict::ImageDict;
#[cfg(feature = "jbig2")]
pub use jbig2::decode_jbig2;
#[cfg(feature = "jpeg2000")]
pub(crate) use jpx::SpaceOverride;
#[cfg(feature = "jpeg2000")]
pub use jpx::{JpxImage, decode_jpx};
pub use mask::ImageMask;
pub(crate) use mask::{ColorKey, matte_color};
pub use packed::{Depth, Packed, Unpacked};
pub use rows::{Converted, Palette, Rgb8, Rgba8, Row, Rows, Source};

use crate::color::{ColorSpace, Rgb};
use crate::error::Error;
use crate::function::FunctionCache;
use crate::names;
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
#[cfg(feature = "ccitt")]
use pdfrum_filters::{CcittParams, decode_ccitt};
use pdfrum_filters::{Filter, decode_chain};
use pdfrum_object::{Dict, Object, Resolve, Stream};

/// Largest pixel grid (`width × height`) this crate will materialize from
/// file-declared dimensions.
///
/// The dictionary gate bounds each axis at 131071; 131071 square is inside
/// that bound and is 17 gigapixels. A gigapixel is roughly twice the largest
/// image anything real produces (a 600 dpi A0 scan is 0.56 Gpx). Above it a
/// mask is dropped and a pixmap is not built — see [`image_area_is_workable`].
///
/// The reduction pre-pass and `to_pixmap` use this same predicate, so a size
/// one path refuses the other does not walk.
pub const MAX_IMAGE_PIXELS: u64 = 1 << 30;

/// Whether a file-declared width and height are small enough to convert,
/// reduce, or unpack into a dense plane.
///
/// Both axes come from the file. The product is what has to be asked about.
#[must_use]
pub fn image_area_is_workable(width: u32, height: u32) -> bool {
    u64::from(width).saturating_mul(u64::from(height)) <= MAX_IMAGE_PIXELS
}

/// Decoded pixels, in whichever shape the source produced.
///
/// Keeping the shape rather than always widening to RGB matters: an indexed
/// image's palette is what a renderer needs to resample correctly, and a
/// one-bit stencil is a mask, not a picture.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Pixels {
    /// One bit per pixel, packed MSB-first with byte-aligned rows. A set bit
    /// paints; this is what a stencil mask produces.
    Stencil(BitImage),
    /// Eight-bit grey.
    Gray8(Box<[u8]>),
    /// Eight-bit red, green, blue.
    Rgb8(Box<[u8]>),
    /// Eight-bit cyan, magenta, yellow, black.
    Cmyk8(Box<[u8]>),
    /// Palette indices with the palette to resolve them.
    Indexed {
        /// One index per pixel.
        indices: Box<[u8]>,
        /// The resolved colours, one per index value.
        palette: Box<[Rgb]>,
    },
}

impl Pixels {
    /// How many components each pixel carries.
    #[must_use]
    pub fn components(&self) -> usize {
        match self {
            Self::Stencil(_) | Self::Gray8(_) | Self::Indexed { .. } => 1,
            Self::Rgb8(_) => 3,
            Self::Cmyk8(_) => 4,
        }
    }

    /// Bytes held.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        match self {
            Self::Stencil(b) => b.bits.len(),
            Self::Gray8(d) | Self::Rgb8(d) | Self::Cmyk8(d) => d.len(),
            Self::Indexed { indices, palette } => {
                indices.len() + palette.len() * std::mem::size_of::<Rgb>()
            }
        }
    }
}

/// An image's samples, in whichever state the decode ladder left them.
///
/// The two states are genuinely different things, not one thing with a flag.
/// [`Samples::Packed`] is what every path that does not run a codec of its own
/// produces: the filter chain's bytes, still at the dictionary's
/// `/BitsPerComponent`, which the row pipeline widens as it walks
/// ([`Unpacked`]). [`Samples::Whole`] is what a codec produced — DCT, JPEG
/// 2000 and JBIG2 all hand back eight-bit samples — together with the two
/// arms that cannot be lazy at all: a stencil's bits, and the palettes an
/// indexed or tint image resolves once.
///
/// Nothing widens a packed image until someone asks for whole-image
/// [`Pixels`], which is [`Samples::to_pixels`] and nowhere else.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Samples {
    /// Still packed, walked by [`Unpacked`].
    Packed(Packed),
    /// Already one byte per component, or a shape that has no packed form.
    Whole(Pixels),
}

impl Samples {
    /// How many components each pixel carries.
    #[must_use]
    pub fn components(&self) -> usize {
        match self {
            Self::Packed(p) => p.components(),
            Self::Whole(p) => p.components(),
        }
    }

    /// Bytes held, for the render cache's budget.
    ///
    /// Honest about what is *actually* held: a packed image is its packed
    /// bytes and its decode table, which is what the cache is keeping alive,
    /// and is smaller than the widened form it never builds.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        match self {
            Self::Packed(p) => p.byte_size(),
            Self::Whole(p) => p.byte_size(),
        }
    }

    /// Whether these samples are a stencil mask's bits.
    ///
    /// A stencil never has a packed form — its bits are its representation —
    /// so this is a question about the [`Whole`](Self::Whole) arm alone.
    #[must_use]
    pub const fn is_stencil(&self) -> bool {
        matches!(self, Self::Whole(Pixels::Stencil(_)))
    }

    /// The resolved palette, when these samples are indices into one.
    ///
    /// Only [`Pixels::Indexed`] has one; every other representation carries
    /// its colours in the samples themselves.
    #[must_use]
    pub fn palette(&self) -> Option<&[Rgb]> {
        match self {
            Self::Whole(Pixels::Indexed { palette, .. }) => Some(palette),
            _ => None,
        }
    }

    /// The whole image as [`Pixels`], widening a packed one if it has to.
    ///
    /// The one place the full-size buffer the row pipeline exists to avoid is
    /// built. Three callers want it and no more: the CLI's image export, the
    /// facade's edit path, and a test that names the representation.
    #[must_use]
    pub fn to_pixels(&self) -> Pixels {
        match self {
            Self::Whole(p) => p.clone(),
            Self::Packed(p) => {
                let data = Unpacked::new(p).collect_all();
                match p.components() {
                    1 => Pixels::Gray8(data),
                    4 => Pixels::Cmyk8(data),
                    _ => Pixels::Rgb8(data),
                }
            }
        }
    }
}

/// A fully decoded image.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageData {
    /// Width in samples, from the codec when it disagreed with the
    /// dictionary.
    pub width: u32,
    /// Height in samples.
    pub height: u32,
    /// The samples, in whichever state the decode left them.
    pub samples: Samples,
    /// The alpha, however it was expressed.
    pub mask: Option<ImageMask>,
    /// The `/Matte` colour a pre-blended soft-masked image was composed
    /// against.
    pub matte: Option<Rgb>,
    /// `/Interpolate`, a hint the renderer may honour.
    pub interpolate: bool,
}

impl ImageData {
    /// Bytes held, for the cache's budget.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.samples.byte_size()
            + match &self.mask {
                Some(ImageMask::Alpha { alpha, .. }) => alpha.len(),
                _ => 0,
            }
    }
}

/// Decode an image `XObject`.
///
/// `form_resources` is searched for a named colour space before
/// `page_resources`, and the interpreter passes it only for an inline image:
/// a real `XObject` sees the page's resources alone. `size` says how much
/// resolution the caller needs, which only the DCT and JPEG 2000 codecs act
/// on.
///
/// # Errors
///
/// [`Error::ImageBadDict`] for a dictionary that will not validate,
/// [`Error::ImageNoColorSpace`] when a non-mask image has no usable space,
/// [`Error::ImageUndecodable`] when no codec can produce samples, and
/// [`Error::CodecRejected`] when one tried and failed.
#[expect(
    clippy::too_many_arguments,
    reason = "the image ladder genuinely needs the stream, both resource \
              dictionaries, the requested size, the resolver, the function \
              cache, limits and diagnostics"
)]
#[expect(
    clippy::too_many_lines,
    reason = "the load ladder reads as one sequence; splitting it would hide \
              the order the rungs run in"
)]
pub fn decode_image<R: Resolve>(
    stream: &Stream,
    form_resources: Option<&Dict>,
    page_resources: Option<&Dict>,
    size: RequestedSize,
    r: &R,
    functions: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<ImageData, Error> {
    let info = ImageDict::load(&stream.dict, r, diags)?;

    // A stencil mask needs no colour space at all.
    if info.image_mask {
        return decode_stencil(stream, &info, r, limits, diags);
    }

    let space = resolve_space(
        &stream.dict,
        form_resources,
        page_resources,
        r,
        functions,
        limits,
        diags,
    );
    let components = info
        .components
        .max(u32::try_from(space.as_ref().map_or(0, ColorSpace::n_components)).unwrap_or(0));
    let info = ImageDict { components, ..info };

    let decoded = decode_chain(stream, info.total_bytes().unwrap_or(0), r, limits, diags);

    // The codecs, dispatched on the last filter. The requested size reaches
    // only the JPEG 2000 decoder, which is the one that carries a pyramid.
    #[cfg(not(feature = "jpeg2000"))]
    let _ = size;
    let (width, height, samples, jpx_alpha) = match info.last_filter {
        #[cfg(feature = "jpeg2000")]
        Some(Filter::Jpx) => {
            let smask_in_data = stream.dict.int(names::SMASK_IN_DATA, r).unwrap_or(0);
            // The request goes to the codec whole rather than as a level
            // count: JPEG 2000 carries the pyramid, so the decoder is the one
            // that knows how many levels it has to give.
            let image = decode_jpx(&decoded.data, space.as_ref(), smask_in_data, size, limits)
                .inspect_err(|_| {
                    diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
                })?;
            if image.space_override != SpaceOverride::Keep {
                diags.record(Severity::Recovered, DiagKind::JpxColorSpaceOverride, None);
            }
            let pixels = match (&space, image.components) {
                // An `/Indexed` space keeps its indices and a resolved
                // palette. The decoder was asked for raw indices rather than
                // colours (`JpxAction::UseIndexed`), but it still hands them
                // back as **eight-bit** samples, so a `/BitsPerComponent`
                // below eight has to be shifted back down:
                //
                // ```
                // } else if (color_space_ && family == kIndexed && bpc_ < 8) {
                //   int scale = 8 - bpc_;
                //   for (auto& pixel : scanline) { pixel >>= scale; }
                // }
                // ```
                //
                // Without it every sample overshoots the palette and clamps
                // to its last entry; with the palette dropped altogether the
                // indices themselves reach the page as grey, which is what
                // `jpxdecode_indexed.in` rendered before.
                (Some(cs @ ColorSpace::Indexed(indexed)), 1) => {
                    // The shift runs only for `bpc_ < 8`, so a declared depth
                    // of eight or more leaves the samples alone. A *zero*
                    // depth is the no-colour-space JPX path, where PDFium
                    // would shift by eight and clear every index; we keep the
                    // same answer without the overflowing shift.
                    let indices: Box<[u8]> = if info.bpc >= 8 {
                        image.data.iter().copied().collect()
                    } else {
                        let scale = 8u32.saturating_sub(info.bpc);
                        image
                            .data
                            .iter()
                            .map(|&v| u8::try_from(u32::from(v) >> scale).unwrap_or(0))
                            .collect()
                    };
                    let palette = (0..=indexed.max_index)
                        .map(|i| cs.to_rgb(&[f32::from(i)]))
                        .collect();
                    Pixels::Indexed { indices, palette }
                }
                (_, 1) => Pixels::Gray8(image.data.into()),
                (_, 4) => Pixels::Cmyk8(image.data.into()),
                _ => Pixels::Rgb8(image.data.into()),
            };
            (
                image.width,
                image.height,
                Samples::Whole(pixels),
                image.alpha,
            )
        }
        #[cfg(not(feature = "jpeg2000"))]
        Some(Filter::Jpx) => {
            diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
            return Err(Error::ImageUndecodable {
                what: concat!("this build has no ", "Jpx", " decoder (feature `jpeg2000`)"),
            });
        }
        #[cfg(feature = "jbig2")]
        Some(Filter::Jbig2) => {
            let globals = info
                .params
                .stream(names::JBIG2_GLOBALS, r)
                // Absent or unfetchable globals are silently tolerated.
                .map(|s| decode_chain(&s, 0, r, limits, diags).data);
            let bits = decode_jbig2(
                globals.as_deref(),
                &decoded.data,
                info.width,
                info.height,
                limits,
            )
            .inspect_err(|_| {
                diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
            })?;
            // A JBIG2 codestream is bi-level, but that does not make every
            // JBIG2 image a *stencil*. One that declares a `/ColorSpace` and
            // does not declare `/ImageMask` is an ordinary one-bit picture,
            // and PDFium draws it as one: `Jbig2Decoder::Decode` finishes with
            // `pix = ~pix` over the whole buffer (`jbig2_decoder.cpp:33`),
            // turning JBIG2's "1 means black" into the PDF sample convention
            // where 0 is black, and the result then goes through the ordinary
            // colour-space and `/Decode` path like any other 1-bit image.
            //
            // The bit layout is already right: a one-bit, one-component row is
            // `width.div_ceil(8)` bytes, which is exactly `ImageDict::pitch`.
            if info.image_mask {
                (
                    info.width,
                    info.height,
                    Samples::Whole(Pixels::Stencil(bits)),
                    None,
                )
            } else {
                let mut samples = bits.bits;
                for byte in &mut samples {
                    *byte = !*byte;
                }
                let samples = unpack(&info, space.as_ref(), &samples, diags)?;
                (info.width, info.height, samples, None)
            }
        }
        #[cfg(not(feature = "jbig2"))]
        Some(Filter::Jbig2) => {
            diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
            return Err(Error::ImageUndecodable {
                what: concat!("this build has no ", "Jbig2", " decoder (feature `jbig2`)"),
            });
        }
        Some(Filter::Dct) => {
            let image = decode_dct(&decoded.data, (info.width, info.height)).inspect_err(|_| {
                diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
            })?;
            // The codec's dimensions **override** the dictionary's.
            if image.width != info.width || image.height != info.height {
                diags.record(
                    Severity::Recovered,
                    DiagKind::ImageDimensionsFromCodec,
                    None,
                );
            }
            if !dct::component_mismatch_allowed(space.as_ref(), image.components) {
                return Err(Error::ImageUndecodable {
                    what: "JPEG component count disagrees with the colour space",
                });
            }
            let mut data = image.data;
            apply_codec_decode(&mut data, space.as_ref(), image.components, &info);
            let pixels = match image.components {
                1 => Pixels::Gray8(data.into()),
                4 => Pixels::Cmyk8(data.into()),
                _ => Pixels::Rgb8(data.into()),
            };
            (image.width, image.height, Samples::Whole(pixels), None)
        }
        // A one-bit fax image with a colour space of its own: the bits are the
        // samples, so they go through `unpack` like any other 1-bit picture and
        // pick up `/Decode` and the palette on the way.
        Some(Filter::CcittFax) => {
            let samples = ccitt_samples(&info, &decoded.data, r, limits, diags)?;
            let samples = unpack(&info, space.as_ref(), &samples, diags)?;
            (info.width, info.height, samples, None)
        }
        _ => {
            if decoded.image.is_some() && info.last_filter.is_none() {
                return Err(Error::ImageUndecodable {
                    what: "an unrecognised filter left no decoder",
                });
            }
            let samples = unpack(&info, space.as_ref(), &decoded.data, diags)?;
            (info.width, info.height, samples, None)
        }
    };

    // `/SMask` wins over `/Mask` at every level: when it is present the
    // colour-key array is never even read.
    let mask = load_mask(
        &stream.dict,
        &info,
        space.as_ref(),
        jpx_alpha,
        r,
        functions,
        limits,
        diags,
    );

    // A colour key is a *predicate on raw samples*, so it has to be resolved
    // while they are still in hand. PDFium does this inside `GetScanline`,
    // writing `alpha = out_of_range ? 0xFF : 0` beside each pixel; here the
    // pixels are already unpacked, so it is one more pass over the same
    // scanlines. See [`resolve_color_key`].
    let mask = match mask {
        Some(ImageMask::ColorKey(key)) => {
            resolve_color_key(&key, &info, &decoded.data, width, height)
        }
        other => other,
    };

    let matte = matte_color(
        stream.dict.array(names::MATTE, r).as_ref(),
        space.as_ref(),
        usize::try_from(info.components).unwrap_or(0),
    );

    Ok(ImageData {
        width,
        height,
        samples,
        mask,
        matte,
        interpolate: stream.dict.bool(names::INTERPOLATE).unwrap_or(false),
    })
}

/// Turn a colour key into the alpha plane it implies.
///
/// A `/Mask` array names, per component, a closed range of **raw sample**
/// values that is transparent — so the test cannot be run on the decoded
/// colours, and it cannot be run at draw time either, because by then the
/// samples are gone. It runs at unpack time instead, beside the scanline that
/// still has the raw samples, and produces an alpha plane where **a sample
/// inside the named range is transparent** — opaque everywhere else.
/// `bug_343075986.in` masks index 0
/// out of an indexed image so a yellow background shows through; without this
/// the index paints its palette entry, which is black.
///
/// Returns `None` when the key covers nothing, which leaves the image opaque
/// rather than inventing a fully-opaque plane to carry.
fn resolve_color_key(
    key: &ColorKey,
    info: &ImageDict,
    data: &[u8],
    width: u32,
    height: u32,
) -> Option<ImageMask> {
    let components = usize::try_from(info.components).unwrap_or(0);
    if components == 0 || info.bpc == 0 || key.ranges.is_empty() {
        return None;
    }
    let pitch = info.pitch()?;
    let pixels_per_row = usize::try_from(width).ok()?;
    let rows = usize::try_from(height).ok()?;
    let mut alpha = vec![255u8; pixels_per_row.checked_mul(rows)?];
    let mut samples = vec![0u32; components];
    let mut any = false;
    for y in 0..rows {
        let (line, availability) = scanline::scanline(data, u32::try_from(y).unwrap_or(0), pitch);
        // An absent row's samples are all zero, and PDFium's zeroed-output arm
        // returns before the colour-key pass too, so it stays opaque.
        if availability == scanline::Availability::Absent {
            continue;
        }
        for x in 0..pixels_per_row {
            for (c, slot) in samples.iter_mut().enumerate() {
                let bit_pos = (x * components + c) * info.bpc as usize;
                *slot = scanline::get_bits(&line, bit_pos, info.bpc);
            }
            if key.is_transparent(&samples)
                && let Some(a) = alpha.get_mut(y * pixels_per_row + x)
            {
                *a = 0;
                any = true;
            }
        }
    }
    any.then(|| ImageMask::Alpha {
        width,
        height,
        alpha: alpha.into(),
        stencil: false,
    })
}

/// Whether the samples are read straight out of the stream, with no image
/// codec standing between it and the scanline.
///
/// This is the precondition of the zeroed-output arm: a row past the end of
/// the data reads as all zeros only when the samples came straight from the
/// stream. A codec — JBIG2, JPX, DCT, CCITT, or a Flate/RunLength predictor
/// built as a scanline decoder — always hands back a full row, so the empty
/// case never arises there and every row is decoded normally. Only a stream
/// read directly can run out. Applying the zeroed arm to a codec's output
/// instead makes a truncated JBIG2 mask stop inverting partway down, which is
/// what `bug_674771.in` showed.
fn reads_the_stream_directly(info: &ImageDict) -> bool {
    !matches!(
        info.last_filter,
        Some(Filter::Jbig2 | Filter::Jpx | Filter::Dct | Filter::CcittFax)
    )
}

/// A stencil mask: one bit per pixel, inverted when the decode is the
/// default.
fn decode_stencil<R: Resolve>(
    stream: &Stream,
    info: &ImageDict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<ImageData, Error> {
    let total = info.total_bytes().ok_or(Error::ImageTooLarge)?;
    let decoded = decode_chain(stream, total, r, limits, diags);
    let row_bytes = info.pitch().ok_or(Error::ImageTooLarge)?;

    // A stencil is still allowed to be JBIG2-coded, and then the codestream —
    // not the stream's own bytes — is what carries the bits. Missing that
    // makes a compressed codestream get read as if it were already one bit per
    // pixel: `bug_527174.pdf`'s single data byte `0x30` unpacked to a set bit
    // and painted a solid black square where the codec should have refused the
    // image outright. Nothing else reaches this rung with an image codec in
    // front, because every other one needs a colour space and a colour space
    // means this is not a stencil.
    if info.last_filter == Some(Filter::Jbig2) {
        return stencil_from_jbig2(stream, info, &decoded.data, r, limits, diags);
    }

    // A fax-coded stencil is the same story: the codestream, not the stream's
    // own bytes, carries the bits. Its samples arrive in the ordinary sense —
    // a set bit is white — so from here they take the ordinary decode, which
    // is the inversion below.
    let ccitt = if info.last_filter == Some(Filter::CcittFax) {
        Some(ccitt_samples(info, &decoded.data, r, limits, diags)?)
    } else {
        None
    };
    let samples = ccitt.as_deref().unwrap_or(&decoded.data);

    let mut bits = vec![0u8; total];
    let mut padded = false;
    let raw = reads_the_stream_directly(info);
    for y in 0..info.height {
        let (mut line, availability) = scanline::scanline(samples, y, row_bytes);
        padded |= availability != scanline::Availability::Whole;
        // The default decode **inverts**; `/Decode [1 0]` copies verbatim —
        // except on a row the stream never reached, which skips the decode
        // altogether and comes back zero. See [`reads_the_stream_directly`]
        // for why that only applies when there is no image codec in front.
        if info.default_decode && !(raw && availability == scanline::Availability::Absent) {
            scanline::invert_line(&mut line);
        }
        let start = usize::try_from(y).unwrap_or(0).saturating_mul(row_bytes);
        if let Some(dest) = bits.get_mut(start..start + row_bytes) {
            dest.copy_from_slice(&line);
        }
    }
    if padded {
        diags.record(Severity::Recovered, DiagKind::ImageStreamTruncated, None);
    }
    Ok(ImageData {
        width: info.width,
        height: info.height,
        samples: Samples::Whole(Pixels::Stencil(BitImage {
            width: info.width,
            height: info.height,
            row_bytes,
            bits,
        })),
        mask: None,
        matte: None,
        interpolate: stream.dict.bool(names::INTERPOLATE).unwrap_or(false),
    })
}

#[cfg(feature = "ccitt")]
/// Decode a `/CCITTFaxDecode` image into the sample buffer the rest of the
/// image path expects.
///
/// Two conventions have to line up, and only one of them needs work.
///
/// The **bit sense already matches**: the fax decoder fills a row with white
/// and clears bits for black, and `/BlackIs1` inverts — which is exactly what
/// a one-bit `/DeviceGray` sample means, so the bits are the samples with no
/// translation. (This is the same pairing the JBIG2 *stencil* rung relies on,
/// and the opposite of the JBIG2 *sample* rung, which inverts because JBIG2
/// sets a bit for black.)
///
/// The **row stride does not**. A fax row is padded to four bytes, because
/// that is the decoder's buffer shape; every consumer here reads rows at
/// [`ImageDict::pitch`], which is `width.div_ceil(8)`. For any width that is
/// not a multiple of 32 the two differ, and reading the wide buffer at the
/// narrow stride shears the image progressively — each row starting a few
/// pixels further into the previous one. Repacking is what this function is
/// mostly for.
fn ccitt_samples<R: Resolve>(
    info: &ImageDict,
    data: &[u8],
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<Vec<u8>, Error> {
    let params = CcittParams::from_dict(&info.params, r);
    let image =
        decode_ccitt(data, params, info.width, info.height, limits, diags).map_err(|_| {
            diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
            Error::ImageUndecodable {
                what: "CCITT fax data would not decode",
            }
        })?;
    let pitch = info.pitch().ok_or(Error::ImageTooLarge)?;
    let total = info.total_bytes().ok_or(Error::ImageTooLarge)?;
    // The repacked buffer is a second image the size of the first, and
    // `/Width` and `/Height` are the stream's to declare. The same budget that
    // bounds the decoder's buffer bounds this one.
    if total > limits.max_decoded_stream_len {
        return Err(Error::ImageTooLarge);
    }
    // White, so a row the decoder never produced — a stream that stops short of
    // the declared height — reads as blank rather than as black. The decoder
    // pre-fills its own rows the same way.
    let mut out = vec![0xffu8; total];
    for y in 0..info.height {
        let src = usize::try_from(y)
            .ok()
            .and_then(|y| y.checked_mul(image.row_bytes));
        let dest = usize::try_from(y).ok().and_then(|y| y.checked_mul(pitch));
        let (Some(src), Some(dest)) = (src, dest) else {
            continue;
        };
        let copy = pitch.min(image.row_bytes);
        let (Some(from), Some(to)) = (
            image.bits.get(src..src.saturating_add(copy)),
            out.get_mut(dest..dest.saturating_add(copy)),
        ) else {
            continue;
        };
        to.copy_from_slice(from);
    }
    Ok(out)
}
#[cfg(not(feature = "ccitt"))]
fn ccitt_samples<R: Resolve>(
    info: &ImageDict,
    data: &[u8],
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<Vec<u8>, Error> {
    let _ = (info, data, r, limits);
    diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
    Err(Error::ImageUndecodable {
        what: "this build has no CCITT fax decoder (feature `ccitt`)",
    })
}

#[cfg(feature = "jbig2")]
/// A stencil whose bits come out of a JBIG2 codestream.
///
/// A codestream that will not decode is **fatal to the image**, not something
/// to paint around: PDFium tears the half-built bitmap down and reports the
/// load as failed, so nothing at all is drawn. That is why a `/JBIG2Globals`
/// stream of binary garbage makes a whole image vanish even though the image
/// itself is one pixel — the globals are parsed first and their failure is the
/// image's failure. Returning `Err` here reaches the same place: the builder
/// drops the object.
///
/// The bit sense already matches. JBIG2 sets a bit for a black pixel and a
/// stencil paints where a bit is set, which is exactly the pairing the default
/// `/Decode` asks for; `/Decode [1 0]` reverses the meaning of the samples and
/// so flips every bit.
fn stencil_from_jbig2<R: Resolve>(
    stream: &Stream,
    info: &ImageDict,
    data: &[u8],
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<ImageData, Error> {
    let globals = info
        .params
        .stream(names::JBIG2_GLOBALS, r)
        // Absent or unfetchable globals are silently tolerated; globals that
        // are present but will not parse are not, and fail inside the codec.
        .map(|s| decode_chain(&s, 0, r, limits, diags).data);
    let mut image = decode_jbig2(globals.as_deref(), data, info.width, info.height, limits)
        .inspect_err(|_| {
            diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
        })?;
    if !info.default_decode {
        for byte in &mut image.bits {
            *byte = !*byte;
        }
    }
    Ok(ImageData {
        width: info.width,
        height: info.height,
        samples: Samples::Whole(Pixels::Stencil(image)),
        mask: None,
        matte: None,
        interpolate: stream.dict.bool(names::INTERPOLATE).unwrap_or(false),
    })
}
#[cfg(not(feature = "jbig2"))]
fn stencil_from_jbig2<R: Resolve>(
    stream: &Stream,
    info: &ImageDict,
    data: &[u8],
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<ImageData, Error> {
    let _ = (stream, info, data, r, limits);
    diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
    Err(Error::ImageUndecodable {
        what: "this build has no JBIG2 decoder (feature `jbig2`)",
    })
}

/// The colour space, consulting form resources only for an inline image.
fn resolve_space<R: Resolve>(
    dict: &Dict,
    form_resources: Option<&Dict>,
    page_resources: Option<&Dict>,
    r: &R,
    functions: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<ColorSpace> {
    let cs_obj = dict.raw(names::COLOR_SPACE)?;
    // Form resources first when there are any, then the page's.
    form_resources
        .and_then(|res| {
            crate::color::load_colorspace(cs_obj, Some(res), r, functions, limits, diags)
        })
        .or_else(|| {
            crate::color::load_colorspace(cs_obj, page_resources, r, functions, limits, diags)
        })
}

/// The sample geometry [`unpack`] derives once and its helpers re-read, kept
/// together so a helper takes one argument rather than five positional
/// `usize`s that are trivial to transpose.
struct SampleLayout {
    /// Bytes per source row.
    pitch: usize,
    /// Samples across.
    pixels_per_row: usize,
    /// Rows down.
    rows: usize,
    /// `pixels_per_row * rows`, already checked for overflow.
    total_pixels: usize,
    /// The largest value the sample depth can express.
    max_raw: u32,
}

/// Read a one-component image's raw samples into one byte each.
///
/// `remap` is the `/Decode` mapping when the sample *is* the palette index
/// and the array therefore remaps the index itself — which is the `Indexed`
/// case — and `None` when the mapping belongs in the palette instead, which
/// is [`tint_palette`]'s.
///
/// An absent row is left at zero: PDFium returns a zeroed *output* buffer
/// without ever reaching the decode, so the index stays zero whatever
/// `/Decode` maps a zero sample to. See [`scanline::Availability`].
fn scan_indices(
    info: &ImageDict,
    data: &[u8],
    remap: Option<&DecodeMap>,
    layout: &SampleLayout,
    diags: &mut Diagnostics,
) -> Box<[u8]> {
    let mut indices = vec![0u8; layout.total_pixels];
    let mut padded = false;
    for y in 0..layout.rows {
        let (line, availability) =
            scanline::scanline(data, u32::try_from(y).unwrap_or(0), layout.pitch);
        padded |= availability != scanline::Availability::Whole;
        if availability == scanline::Availability::Absent {
            continue;
        }
        for x in 0..layout.pixels_per_row {
            let raw = scanline::get_bits(&line, x * info.bpc as usize, info.bpc);
            let index = match remap {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the clamp bounds the value to a palette index"
                )]
                Some(decode) => decode.apply(0, f64_to_f32(raw)).clamp(0.0, 255.0) as u8,
                None => u8::try_from(raw.min(255)).unwrap_or(u8::MAX),
            };
            if let Some(slot) = indices.get_mut(y * layout.pixels_per_row + x) {
                *slot = index;
            }
        }
    }
    if padded {
        diags.record(Severity::Recovered, DiagKind::ImageStreamTruncated, None);
    }
    indices.into()
}

/// Resolve a one-component tint image into a palette and its indices.
///
/// For the families that have no device reading the tint transform runs once
/// per distinct
/// sample value rather than once per pixel, which is both exact — a sample of
/// at most eight bits has at most 256 values — and the shape
/// `pdfrum_render::image::to_pixmap` already has a fast path for.
fn tint_palette(
    info: &ImageDict,
    space: &ColorSpace,
    data: &[u8],
    decode: &DecodeMap,
    layout: &SampleLayout,
    diags: &mut Diagnostics,
) -> Pixels {
    // The `/Decode` mapping belongs in the palette rather than on the index,
    // because here the sample is a *tint* the transform consumes rather than
    // a position in a table.
    let indices = scan_indices(info, data, None, layout, diags);
    // The palette spans every value the sample depth can express, with the
    // mapping folded into each entry exactly as `LoadPalette` folds
    // `decode_min_ + decode_step_ * i` into its own.
    let entries = usize::try_from(layout.max_raw).unwrap_or(255).min(255) + 1;
    let palette = (0..entries)
        .map(|i| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "an index of at most 255 is exact in f32"
            )]
            let value = decode.apply(0, i as f32);
            space.to_rgb(&[value])
        })
        .collect();
    Pixels::Indexed { indices, palette }
}

/// Resolve a multi-colorant tint image one pixel at a time.
///
/// Reached the way the oracle reaches its own equivalent: the default-decode
/// shortcut keeps only `DeviceRGB`/`CalRGB` and hands every other family to
/// the bulk translation, whose generic base is the scalar conversion per
/// pixel. A `DeviceN` over more than one colorant has too wide a sample tuple
/// to tabulate, so there is no palette to build.
///
/// The conversion itself is [`ColorSpace::translate_image_line`], which was
/// ported whole and until now had no caller in the image build at all — only
/// a panic test. That absent call site is the actual defect: the arithmetic
/// was always here, `unpack` simply never asked for it.
///
/// `samples` holds the `/Decode`-mapped components as bytes in the space's
/// own component order. The port writes **B, G, R** because that is the
/// device order PDFium's scanline is in; `Pixels::Rgb8` wants R, G, B, so the
/// triples are swapped on the way out rather than by giving the port a second
/// byte order to maintain.
fn tint_per_pixel(
    space: &ColorSpace,
    samples: &[u8],
    total_pixels: usize,
) -> Result<Pixels, Error> {
    let mut bgr = vec![0u8; total_pixels.checked_mul(3).ok_or(Error::ImageTooLarge)?];
    space.translate_image_line(&mut bgr, samples, total_pixels, false);
    for px in bgr.as_chunks_mut::<3>().0 {
        px.swap(0, 2);
    }
    Ok(Pixels::Rgb8(bgr.into()))
}

/// Prepare raw or losslessly-filtered samples for the row pipeline.
///
/// The two families that cannot be walked lazily resolve here and return
/// [`Samples::Whole`]: an indexed or single-colorant tint image, whose samples
/// are *positions in a palette* the caller must be handed with them, and a
/// multi-colorant `DeviceN`, whose tint transform is per pixel and has no
/// table. Everything else -- which is the common case, and the whole of the
/// guide -- becomes a [`Packed`] the row stages widen as they walk it.
fn unpack(
    info: &ImageDict,
    space: Option<&ColorSpace>,
    data: &[u8],
    diags: &mut Diagnostics,
) -> Result<Samples, Error> {
    let space = space.ok_or(Error::ImageNoColorSpace)?;
    let components = usize::try_from(info.components).unwrap_or(0);
    if components == 0 || info.bpc == 0 {
        return Err(Error::ImageUndecodable {
            what: "zero components or bit depth",
        });
    }
    let pitch = info.pitch().ok_or(Error::ImageTooLarge)?;
    let pixels_per_row = usize::try_from(info.width).unwrap_or(0);
    let rows = usize::try_from(info.height).unwrap_or(0);
    let total_pixels = pixels_per_row
        .checked_mul(rows)
        .ok_or(Error::ImageTooLarge)?;

    let decode = DecodeMap::new(Some(space), components, info.bpc, info.decode.as_ref());
    let max_raw = if info.bpc >= 32 {
        u32::MAX
    } else {
        (1u32 << info.bpc) - 1
    };

    let layout = SampleLayout {
        pitch,
        pixels_per_row,
        rows,
        total_pixels,
        max_raw,
    };

    // An indexed image keeps its indices and a resolved palette, which is
    // what a renderer needs to resample it without blending indices. An
    // `/Decode` on one remaps the **index** itself, so it is applied during
    // the scan and the palette is the space's own.
    if let ColorSpace::Indexed(indexed) = space {
        let indices = scan_indices(info, data, Some(&decode), &layout, diags);
        let palette = (0..=indexed.max_index)
            .map(|i| space.to_rgb(&[f32::from(i)]))
            .collect();
        return Ok(Samples::Whole(Pixels::Indexed { indices, palette }));
    }

    // A `Separation` or `DeviceN` sample is a **tint**, not a colour, so it
    // has to be run through the tint transform before it means anything.
    // Widening it to a byte and handing it to the device reading of the
    // component count paints the tint itself — one colorant becomes a grey
    // level, four become a CMYK tuple. See
    // [`ColorSpace::needs_image_conversion`] for why these two families and
    // no others.
    //
    // PDFium reaches the conversion from two directions and both end at
    // `GetRGB`. When `bpc_ * components_ <= 8` — which covers every eight-bit
    // single-component image — `CPDF_DIB::LoadPalette`
    // (`cpdf_dib.cpp:894-979`) precomputes `GetRGB` over all `1 << bits`
    // possible sample values and the image becomes a palette lookup.
    // Anything wider takes `TranslateScanline24bpp` (`:1007-1054`), whose
    // default-decode shortcut (`:1056-1075`) hands every non-RGB family to
    // `TranslateImageLine`; the generic base (`cpdf_colorspace.cpp:636-660`)
    // is `GetRGB` per pixel again.
    //
    // For a single component both collapse to the same thing here: resolve
    // the colour once per distinct sample value into a palette, which is
    // exact (there are at most 256 of them) and is the shape the renderer's
    // indexed fast path already consumes. `corpus/fx/other/1.pdf` is the
    // fixture — a 1x1 `/Separation` image whose lone `0xC6` sample is a 0.776
    // tint of PANTONE 327 CV, teal `(0, 182, 162)` through the tint transform
    // and grey `(198, 198, 198)` without it.
    if components == 1 && space.needs_image_conversion() {
        return Ok(Samples::Whole(tint_palette(
            info, space, data, &decode, &layout, diags,
        )));
    }

    // A multi-colorant `DeviceN` cannot be tabulated -- its sample tuple is
    // too wide -- so it takes `TranslateScanline24bpp`'s own shape instead,
    // which is per pixel and therefore eager.
    if space.needs_image_conversion() {
        let out = widen_whole(info, data, &decode, &layout, diags)?;
        return Ok(Samples::Whole(tint_per_pixel(space, &out, total_pixels)?));
    }

    // Everything else stays packed. The `/Decode` mapping is folded into the
    // table `Packed` builds, and the widening itself happens one row at a time
    // inside [`Unpacked`] as the pipeline pulls.
    let depth = Depth::new(info.bpc).ok_or(Error::ImageUndecodable {
        what: "a bit depth that is not 1, 2, 4, 8 or 16",
    })?;
    let packed = Packed::with_map(
        data.into(),
        depth,
        components,
        pitch,
        info.width,
        info.height,
        &decode,
    );
    // The eager pass recorded this while it walked; a lazy one answers the
    // same question from the stream length, which is the same answer.
    if packed.truncated() {
        diags.record(Severity::Recovered, DiagKind::ImageStreamTruncated, None);
    }
    Ok(Samples::Packed(packed))
}

/// Widen every sample to a byte, over the whole image.
///
/// The one caller left is the multi-colorant tint path, whose transform is per
/// pixel over a component tuple and so has to see the whole thing at once.
fn widen_whole(
    info: &ImageDict,
    data: &[u8],
    decode: &DecodeMap,
    layout: &SampleLayout,
    diags: &mut Diagnostics,
) -> Result<Vec<u8>, Error> {
    let components = usize::try_from(info.components).unwrap_or(0);
    let mut out = vec![
        0u8;
        layout
            .total_pixels
            .checked_mul(components)
            .ok_or(Error::ImageTooLarge)?
    ];
    let mut padded = false;
    for y in 0..layout.rows {
        let (line, availability) =
            scanline::scanline(data, u32::try_from(y).unwrap_or(0), layout.pitch);
        padded |= availability != scanline::Availability::Whole;
        // An absent row never reaches `TranslateScanline24bpp`: PDFium returns
        // a zeroed *output* buffer, so the pixels are literal black rather
        // than whatever `/Decode` maps a zero sample to. See `Availability`.
        if availability == scanline::Availability::Absent {
            continue;
        }
        for x in 0..layout.pixels_per_row {
            for c in 0..components {
                let bit_pos = (x * components + c) * info.bpc as usize;
                let raw = scanline::get_bits(&line, bit_pos, info.bpc);
                let value = decode.apply(c, f64_to_f32(raw));
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the clamp bounds the product to 0..=255"
                )]
                let byte = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                if let Some(slot) = out.get_mut((y * layout.pixels_per_row + x) * components + c) {
                    *slot = byte;
                }
            }
        }
    }
    if padded {
        diags.record(Severity::Recovered, DiagKind::ImageStreamTruncated, None);
    }
    Ok(out)
}

/// Apply a non-default `/Decode` to a codec's eight-bit output, in place.
///
/// `TranslateScanline24bpp` runs on the *decoder's* scanline, not only on raw
/// samples, so a `/Decode` array reaches a DCT or JPEG 2000 image exactly as
/// it reaches a Flate one. The codecs set `bpc_ = 8` before the scanline is
/// read, so the mapping is always over `0..=255` here whatever the dictionary
/// declared. `bug_1646` and `bug_718762` are the fixtures: a CMYK JPEG
/// carrying the Adobe inversion as `/Decode [1 0 1 0 1 0 1 0]`, which without
/// this reaches the page as its own negative.
///
/// The default mapping is a no-op by construction, so it is skipped rather
/// than run — which also keeps an `Indexed` codec output (whose indices this
/// mapping does not describe) untouched.
///
/// # It is a table, because the mapping has 256 possible answers per component
///
/// The value written for a byte depends on the component index and the byte,
/// and on nothing else — so the whole mapping is `components * 256` bytes,
/// at most a kibibyte. Evaluating it per sample instead costs an integer
/// division (`i % components`), two bounds-checked slice reads, a float
/// multiply-add, a clamp, a `round` and a cast, on **every byte of the image**.
///
/// On `image_bug_718762` — a 5000x5000 CMYK JPEG whose `/Decode
/// [1 0 1 0 1 0 1 0]` is the Adobe inversion written out, so the
/// `default_decode` short circuit above does not fire — that loop ran over
/// 100,000,000 bytes at 2.9 ns each and was **83% of the whole image decode**,
/// against 17% for `zune_jpeg` itself. The table
/// is built from the same [`DecodeMap::apply`] and the same rounding, so every
/// output byte is identical by construction; only the number of times the
/// arithmetic runs changes.
fn apply_codec_decode(
    data: &mut [u8],
    space: Option<&ColorSpace>,
    components: u8,
    info: &ImageDict,
) {
    let components = usize::from(components);
    if components == 0 || info.default_decode {
        return;
    }
    let Some(space) = space else { return };
    // An indexed space maps indices rather than colour components, and the
    // codec paths that produce indices resolve them through the palette.
    if matches!(space, ColorSpace::Indexed(_)) {
        return;
    }
    let decode = DecodeMap::new(Some(space), components, 8, info.decode.as_ref());
    if decode.default {
        return;
    }
    let table = decode_table(&decode, components);
    // Chunked by component so the row index is a position in the chunk rather
    // than a division: `chunks_mut` gives back the whole trailing partial
    // chunk too, which is what keeps an image whose byte count is not a whole
    // number of pixels mapping exactly as the per-sample loop did.
    for chunk in data.chunks_mut(components) {
        for (component, sample) in chunk.iter_mut().enumerate() {
            if let Some(row) = table.get(component)
                && let Some(mapped) = row.get(usize::from(*sample))
            {
                *sample = *mapped;
            }
        }
    }
}

/// Every answer [`DecodeMap::apply`] can give, one row of 256 per component.
///
/// The rounding is the per-sample loop's, verbatim: **`round`, not the
/// truncation the raw-sample path uses.** PDFium carries these values as floats
/// all the way into the colour conversion and only truncates the *converted*
/// byte; we have to land them back in a byte here, so the encode has to be the
/// one that makes the round trip exact. Truncating instead loses a count on the
/// commonest case of all — the `[1 0]` inversion, where `1 - 253/255` lands a
/// hair under `2/255` and would come back as 1.
fn decode_table(decode: &DecodeMap, components: usize) -> Vec<[u8; 256]> {
    (0..components)
        .map(|component| {
            let mut row = [0u8; 256];
            for (raw, slot) in row.iter_mut().enumerate() {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a table index below 256 is exact in f32"
                )]
                let value = decode.apply(component, raw as f32);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the clamp bounds the product to 0..=255"
                )]
                let byte = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                *slot = byte;
            }
            row
        })
        .collect()
}

/// A raw sample as a float, at the precision the decode arithmetic uses.
#[expect(
    clippy::cast_precision_loss,
    reason = "raw samples cap at sixteen bits, exact in f32"
)]
fn f64_to_f32(raw: u32) -> f32 {
    raw as f32
}

/// Load `/SMask`, then `/Mask`, then the colour key — in that order, and
/// stopping at the first that produces something.
#[expect(
    clippy::too_many_arguments,
    reason = "loading a mask recursively needs the same context the base image did"
)]
fn load_mask<R: Resolve>(
    dict: &Dict,
    info: &ImageDict,
    space: Option<&ColorSpace>,
    jpx_alpha: Option<Vec<u8>>,
    r: &R,
    functions: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<ImageMask> {
    // A JPX image's captured alpha is already a mask.
    if let Some(alpha) = jpx_alpha {
        return Some(ImageMask::Alpha {
            width: info.width,
            height: info.height,
            alpha: alpha.into(),
            stencil: false,
        });
    }
    // `/SMask` first, and its presence means `/Mask` is never consulted.
    if let Some(smask) = dict.stream(names::SMASK, r) {
        return load_mask_image(&smask, false, r, functions, limits, diags);
    }
    match dict.get(names::MASK, r).as_deref() {
        // A `/Mask` stream is a stencil, whose sense is inverted.
        Some(Object::Stream(mask_stream)) => {
            load_mask_image(mask_stream, true, r, functions, limits, diags)
        }
        // A `/Mask` array is a colour key.
        Some(Object::Array(array)) => {
            let components = usize::try_from(info.components).unwrap_or(0);
            if !ColorKey::is_complete(array, components) {
                diags.record(Severity::Suspicious, DiagKind::ColorKeyArrayShort, None);
            }
            let max_raw = if info.bpc >= 32 {
                u32::MAX
            } else {
                (1u32 << info.bpc.max(1)) - 1
            };
            let _ = space;
            Some(ImageMask::ColorKey(ColorKey::from_array(
                array, components, max_raw,
            )))
        }
        _ => None,
    }
}

/// Decode a mask image.
///
/// A mask is loaded **at full resolution, with no resources, and with no mask
/// of its own** — so a four-hundred-pixel mask stays four hundred pixels even
/// beside a fifty-pixel base image, and a mask can never carry a mask.
///
/// A failure here **drops the mask and keeps the base image**; it never fails
/// the image.
fn load_mask_image<R: Resolve>(
    stream: &Stream,
    stencil: bool,
    r: &R,
    functions: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<ImageMask> {
    let decoded = decode_image(
        stream,
        None,
        None,
        // Never resolution-reduced.
        RequestedSize::Full,
        r,
        functions,
        limits,
        diags,
    );
    let Ok(image) = decoded else {
        diags.record(Severity::Recovered, DiagKind::MaskDropped, None);
        return None;
    };
    let Some(alpha) = mask_plane(&image.samples, image.width, image.height) else {
        diags.record(Severity::Recovered, DiagKind::MaskDropped, None);
        return None;
    };
    Some(ImageMask::Alpha {
        width: image.width,
        height: image.height,
        alpha,
        stencil,
    })
}

/// A decoded mask image's samples as the one-byte-per-pixel coverage plane an
/// [`ImageMask::Alpha`] carries.
///
/// A soft mask's alpha is its luminosity and a stencil's is its coverage;
/// both are the **first byte of the converted sample**, so this is one walk
/// of the [`Converted`] row pipeline keeping the red channel.
///
/// This is the one call site in the *build* that walks a whole image, and on
/// a document of soft-masked thumbnails it is the larger of the two:
/// `image_en_fqa` builds 29.8 million mask samples per page and never
/// converts more than four of the base image's.
///
/// A codec's `Gray8` keeps a direct arm because its sample already *is* the
/// alpha — the row pipeline would widen each byte to RGBA only for this to
/// take the first channel back. Every other kind, packed samples included,
/// goes through the rows, which is the same answer:
/// `the_mask_planes_fast_arms_are_the_general_one` pins the equality.
///
/// `None` when the dimensions overflow a `usize`, the area is above
/// [`MAX_IMAGE_PIXELS`], or the allocator will not meet the buffer.
fn mask_plane(samples: &Samples, width: u32, height: u32) -> Option<Box<[u8]>> {
    // Same predicate `to_pixmap` and the reduction pre-pass use. A mask is
    // never resolution-reduced, so without it the dictionary's per-axis
    // limit of 131071 would still ask for 17 GB. `try_reserve_exact` is not
    // a refusal on an overcommit host: Linux lets that reserve succeed, and
    // the `resize` below then zeros 17 GB.
    if !image_area_is_workable(width, height) {
        return None;
    }
    let len = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    // Second line, for a size inside the cap that this allocator still cannot
    // meet. `handle_alloc_error` would abort with no unwind; `None` here is
    // the mask being dropped, which is what this function's contract already
    // says a failure means.
    let mut alpha = Vec::new();
    alpha.try_reserve_exact(len).ok()?;
    if let Samples::Whole(Pixels::Gray8(data)) = samples {
        alpha.extend(data.iter().take(len).copied());
    } else {
        let palette = match samples {
            Samples::Whole(Pixels::Indexed { palette, .. }) => Some(rows::Palette::new(palette)),
            _ => None,
        };
        let mut converted =
            rows::Converted::new(rows::Source::new(samples, width, height), palette);
        while let Some(row) = rows::Rows::next(&mut converted) {
            alpha.extend(row.pixels().iter().map(|px| px.0[0]));
        }
    }
    // A source plane shorter than the image it describes reads as fully
    // transparent past its end, which is what the zero-filled `Vec` the old
    // walk wrote into did for exactly those samples.
    alpha.resize(len, 0);
    Some(alpha.into())
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
        clippy::cast_sign_loss,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{ImageData, Pixels, RequestedSize, Samples, decode_image};
    use crate::color::Rgb;
    use crate::function::FunctionCache;
    use crate::image::BitImage;
    use pdfrum_common::{DiagKind, Diagnostics, Limits};
    use pdfrum_object::{Array, ByteSpan, Dict, Name, NoResolve, Object, Stream};

    fn stream(pairs: Vec<(Name, Object)>, data: &[u8]) -> Stream {
        Stream::new(Dict::from_pairs(pairs), ByteSpan::from(data.to_vec()))
    }

    fn decode(s: &Stream) -> Result<ImageData, crate::Error> {
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        decode_image(
            s,
            None,
            None,
            RequestedSize::Full,
            &NoResolve,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        )
    }

    /// One pixel's converted colour, through the row pipeline.
    ///
    /// The pipeline is row-at-a-time by design, so a test that wants a single
    /// pixel walks to its row and indexes it. Tests are the only caller that
    /// ever wants one pixel — the render path wants all of them, in order.
    fn converted_row(samples: &Samples, width: u32, y: u32) -> Vec<[u8; 3]> {
        let palette = samples.palette().map(super::rows::Palette::new);
        // Only the wanted row is converted. Walking down to row `y` from the
        // top would be quadratic in `y`, and these tests reach for row 9999 to
        // check the out-of-range fallback.
        let mut converted =
            super::rows::Converted::new(super::rows::Source::at_row(samples, width, y), palette);
        super::rows::Rows::next(&mut converted)
            .map(|row| {
                row.pixels()
                    .iter()
                    .map(|px| [px.0[0], px.0[1], px.0[2]])
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One pixel's converted colour, for a test that wants a single one.
    ///
    /// A caller comparing a whole row should take [`converted_row`] once
    /// instead: this rebuilds the row buffer on every call, which is the right
    /// trade for a handful of pixels and the wrong one for thousands.
    fn sample_at(samples: &Samples, x: u32, y: u32, width: u32) -> [u8; 3] {
        converted_row(samples, width, y)
            .get(x as usize)
            .copied()
            .unwrap_or([0, 0, 0])
    }

    #[test]
    fn a_colour_key_becomes_an_alpha_plane_on_the_raw_samples() {
        // `/Mask [0 0]` over a 2x2 eight-bit grey image: the two zero samples
        // go transparent and the rest stay opaque. The predicate runs on the
        // *raw* values, so it is the byte 0 that matches, not the colour.
        let mut mask = Array::default();
        mask.push(Object::Int(0));
        mask.push(Object::Int(0));
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(2)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceGray")),
                ),
                (Name::from("Mask"), Object::Array(mask)),
            ],
            &[0, 200, 0, 255],
        );
        let image = decode(&s).expect("should decode");
        let Some(crate::image::ImageMask::Alpha { alpha, .. }) = image.mask else {
            panic!("expected a resolved alpha plane, got {:?}", image.mask);
        };
        assert_eq!(&*alpha, &[0u8, 255, 0, 255]);
    }

    #[test]
    fn a_colour_key_that_matches_nothing_leaves_the_image_opaque() {
        // No pixel falls in the range, so there is no plane to carry — the
        // image is opaque and says so by having no mask at all.
        let mut mask = Array::default();
        mask.push(Object::Int(7));
        mask.push(Object::Int(9));
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceGray")),
                ),
                (Name::from("Mask"), Object::Array(mask)),
            ],
            &[0, 200],
        );
        assert!(decode(&s).expect("should decode").mask.is_none());
    }

    #[test]
    fn a_row_past_the_end_of_the_stream_skips_the_decode_entirely() {
        // `bug_554151.in` in miniature: a `/Decode` that maps a zero sample
        // to full red, and a stream holding only the first of two rows.
        //
        // The first row decodes: `FF` at four bits is 15, and
        // `1 + (0 - 1) * 15/15` is 0, so it is black. The second row never
        // reaches the decode at all — `CPDF_DIB::GetScanline` hands back a
        // zeroed *output* buffer — so it is also black, and emphatically not
        // the red that decoding a zero sample would give.
        let mut decode_array = Array::default();
        decode_array.push(Object::Real(1.0));
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(2)),
                (Name::from("BitsPerComponent"), Object::Int(4)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceRGB")),
                ),
                (Name::from("Decode"), Object::Array(decode_array)),
            ],
            // One row: two pixels of three four-bit components each.
            &[0xFF, 0xFF, 0xFF],
        );
        let image = decode(&s).expect("should decode");
        assert_eq!(
            image.samples.to_pixels(),
            Pixels::Rgb8(Box::from(&[0u8; 12][..])),
            "both rows are black; the absent one never reaches `/Decode`"
        );
    }

    #[test]
    fn an_eight_bit_grayscale_image_round_trips() {
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(2)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceGray")),
                ),
            ],
            &[0, 85, 170, 255],
        );
        let image = decode(&s).expect("should decode");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(
            image.samples.to_pixels(),
            Pixels::Gray8(Box::from(&[0u8, 85, 170, 255][..]))
        );
        assert!(image.mask.is_none());
    }

    /// The 94-byte JBIG2 codestream from `transfer_function.in`'s
    /// `/IM_1bpp`, a 400x400 image that decodes to solid black.
    const JBIG2_ALL_BLACK: [u8; 94] = [
        0x00, 0x00, 0x00, 0x00, 0x30, 0x00, 0x01, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x0d, 0xea,
        0x00, 0x00, 0x03, 0x53, 0x00, 0x00, 0x17, 0x11, 0x00, 0x00, 0x17, 0x11, 0x51, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x26, 0x00, 0x01, 0x00, 0x00, 0x00, 0x35, 0x00, 0x00, 0x0d, 0xea,
        0x00, 0x00, 0x03, 0x53, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x03,
        0xff, 0xfd, 0xff, 0x02, 0xfe, 0xfe, 0xfe, 0xff, 0x7f, 0x86, 0x53, 0x0f, 0xb6, 0xc9, 0x22,
        0xcf, 0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f,
        0xff, 0x7f, 0xff, 0xac,
    ];

    #[test]
    fn a_jbig2_image_with_a_colour_space_is_a_picture_and_not_a_stencil() {
        // A JBIG2 codestream is bi-level, and it is tempting to conclude that
        // every JBIG2 image is a mask. It is not: this one declares
        // `/DeviceGray` and no `/ImageMask`, so it is an ordinary one-bit
        // picture and the sample convention is the PDF's — 0 is black.
        // Reading it as a stencil paints it in the fill colour wherever
        // JBIG2 said "black", which for an all-black image drawn on a light
        // page is a whole square of the wrong colour.
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(400)),
                (Name::from("Height"), Object::Int(400)),
                (Name::from("BitsPerComponent"), Object::Int(1)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceGray")),
                ),
                (
                    Name::from("Filter"),
                    Object::Name(Name::from("JBIG2Decode")),
                ),
            ],
            &JBIG2_ALL_BLACK,
        );
        let image = decode(&s).expect("should decode");
        assert_eq!((image.width, image.height), (400, 400));
        let pixels = image.samples.to_pixels();
        let Pixels::Gray8(gray) = &pixels else {
            panic!("expected grey samples, got {pixels:?}");
        };
        assert_eq!(gray.len(), 400 * 400);
        assert!(
            gray.iter().all(|&v| v == 0),
            "every sample is black: JBIG2's set bit inverts to sample 0"
        );
    }

    /// A 69-byte embedded JBIG2 codestream: an 8x8 page whose every row is
    /// four white pixels then four black, so each row is `0b0000_1111`. Built
    /// from a page-information segment and one MMR-coded generic region, and
    /// small enough that a test can state the expected bits outright.
    const JBIG2_RIGHT_HALF_BLACK: [u8; 69] = [
        0x00, 0x00, 0x00, 0x00, 0x30, 0x00, 0x01, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x26, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1c, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x36,
        0xcd, 0xb3, 0x6c, 0xdb, 0x36, 0xcd, 0xb3, 0x6c, 0xdb,
    ];

    /// A stencil dictionary over `data`, optionally with a `/Decode` array.
    fn jbig2_stencil(data: &[u8], decode: Option<[i64; 2]>) -> Stream {
        let mut pairs = vec![
            (Name::from("Width"), Object::Int(8)),
            (Name::from("Height"), Object::Int(8)),
            (Name::from("ImageMask"), Object::Bool(true)),
            (
                Name::from("Filter"),
                Object::Name(Name::from("JBIG2Decode")),
            ),
        ];
        if let Some([lo, hi]) = decode {
            pairs.push((
                Name::from("Decode"),
                Object::Array(Array::of([Object::Int(lo), Object::Int(hi)])),
            ));
        }
        stream(pairs, data)
    }

    #[test]
    fn a_jbig2_stencil_takes_its_bits_from_the_codestream() {
        // Without a colour space the image is a stencil, but the bits still
        // come from the codec — reading the compressed bytes as if they were
        // already one bit per pixel paints noise.
        let image = decode(&jbig2_stencil(&JBIG2_RIGHT_HALF_BLACK, None)).expect("should decode");
        let Samples::Whole(Pixels::Stencil(BitImage {
            bits, row_bytes, ..
        })) = &image.samples
        else {
            panic!("expected a stencil, got {:?}", image.samples);
        };
        assert_eq!(*row_bytes, 1);
        assert_eq!(
            &bits[..],
            &[0b0000_1111u8; 8][..],
            "the codestream's black half is where the stencil inks"
        );
    }

    #[test]
    fn a_jbig2_stencil_with_decode_one_zero_flips_every_bit() {
        // `/Decode [1 0]` reverses what a sample means, so the ink lands on
        // the half the codestream left white.
        let image =
            decode(&jbig2_stencil(&JBIG2_RIGHT_HALF_BLACK, Some([1, 0]))).expect("should decode");
        let Samples::Whole(Pixels::Stencil(BitImage { bits, .. })) = &image.samples else {
            panic!("expected a stencil, got {:?}", image.samples);
        };
        assert_eq!(&bits[..], &[0b1111_0000u8; 8][..]);
    }

    #[test]
    fn a_jbig2_stencil_whose_codestream_will_not_decode_is_refused() {
        // The whole image fails rather than being painted from whatever the
        // undecoded bytes happen to look like: PDFium tears the half-built
        // bitmap down and draws nothing at all. `bug_527174.pdf` is this case
        // — a one-byte codestream that, read raw, inverted to a set bit and
        // painted a solid black square.
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        let got = decode_image(
            &jbig2_stencil(b"0", None),
            None,
            None,
            RequestedSize::Full,
            &NoResolve,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        );
        assert!(
            got.is_err(),
            "an undecodable codestream is fatal, got {got:?}"
        );
        assert!(
            diags.contains(&DiagKind::ImageDecodeFailed),
            "the refusal is recorded, not silent: {:?}",
            diags.entries()
        );
    }

    #[test]
    fn a_stencil_mask_with_the_default_decode_is_inverted() {
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(8)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("ImageMask"), Object::Bool(true)),
            ],
            &[0b1010_1010],
        );
        let image = decode(&s).expect("should decode");
        let Samples::Whole(Pixels::Stencil(BitImage { bits, .. })) = &image.samples else {
            panic!("expected a stencil, got {:?}", image.samples);
        };
        assert_eq!(bits.first(), Some(&0b0101_0101));
    }

    #[test]
    fn a_stencil_mask_with_decode_one_zero_is_copied_verbatim() {
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(8)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("ImageMask"), Object::Bool(true)),
                (
                    Name::from("Decode"),
                    Object::Array(Array::of([Object::Int(1), Object::Int(0)])),
                ),
            ],
            &[0b1010_1010],
        );
        let image = decode(&s).expect("should decode");
        let Samples::Whole(Pixels::Stencil(BitImage { bits, .. })) = &image.samples else {
            panic!("expected a stencil");
        };
        assert_eq!(bits.first(), Some(&0b1010_1010));
    }

    #[test]
    fn a_truncated_stream_is_zero_padded_rather_than_rejected() {
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(2)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceGray")),
                ),
            ],
            // Two bytes short of four.
            &[10, 20],
        );
        let image = decode(&s).expect("should still decode");
        assert_eq!(
            image.samples.to_pixels(),
            Pixels::Gray8(Box::from(&[10u8, 20, 0, 0][..]))
        );
    }

    #[test]
    fn an_indexed_image_keeps_its_indices_and_a_palette() {
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(4)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("BitsPerComponent"), Object::Int(2)),
                (
                    Name::from("ColorSpace"),
                    Object::Array(Array::of([
                        Object::Name(Name::from("Indexed")),
                        Object::Name(Name::from("DeviceGray")),
                        Object::Int(3),
                        Object::Str(pdfrum_object::PdfString::literal([0u8, 85, 170, 255])),
                    ])),
                ),
            ],
            // Four two-bit indices: 0, 1, 2, 3.
            &[0b00_01_10_11],
        );
        let image = decode(&s).expect("should decode");
        let Samples::Whole(Pixels::Indexed { indices, palette }) = &image.samples else {
            panic!("expected indexed pixels, got {:?}", image.samples);
        };
        assert_eq!(&**indices, &[0, 1, 2, 3]);
        assert_eq!(palette.len(), 4);
        assert!(palette[0].r.abs() < 1e-6);
        assert!((palette[3].r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_bad_bit_depth_is_an_error_rather_than_a_repair() {
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(2)),
                (Name::from("BitsPerComponent"), Object::Int(3)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceGray")),
                ),
            ],
            &[0; 16],
        );
        assert!(decode(&s).is_err());
    }

    #[test]
    fn a_colour_key_mask_is_read_from_a_mask_array() {
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    Object::Name(Name::from("DeviceGray")),
                ),
                (
                    Name::from("Mask"),
                    Object::Array(Array::of([Object::Int(0), Object::Int(10)])),
                ),
            ],
            &[5, 200],
        );
        // The array is read as a `ColorKey` and then *resolved* against the
        // raw samples before the image leaves this crate: sample 5 falls in
        // `0..=10` and goes transparent, 200 does not and stays opaque. The
        // key itself never reaches a renderer, because by draw time the raw
        // samples the predicate needs are gone.
        let image = decode(&s).expect("should decode");
        let Some(super::ImageMask::Alpha { alpha, .. }) = &image.mask else {
            panic!("expected a resolved alpha plane, got {:?}", image.mask);
        };
        assert_eq!(&**alpha, &[0u8, 255]);
        // The predicate itself still reads the way the array wrote it.
        let key =
            super::ColorKey::from_array(&Array::of([Object::Int(0), Object::Int(10)]), 1, 255);
        assert!(key.is_transparent(&[5]));
        assert!(!key.is_transparent(&[200]));
    }

    #[test]
    fn pixel_lookup_is_bounds_checked() {
        let pixels = Samples::Whole(Pixels::Rgb8(Box::from(&[255u8, 0, 0, 0, 255, 0][..])));
        assert_eq!(sample_at(&pixels, 0, 0, 2), [255, 0, 0]);
        // Out of range reads as black rather than panicking.
        assert_eq!(sample_at(&pixels, 99, 99, 2), [0, 0, 0]);
        assert_eq!(pixels.components(), 3);
    }

    /// An image dictionary for the codec `/Decode` tests.
    fn codec_dict(space: &str, decode: Option<Vec<f32>>) -> super::ImageDict {
        let mut pairs = vec![
            (Name::from("Width"), Object::Int(2)),
            (Name::from("Height"), Object::Int(1)),
            (Name::from("BitsPerComponent"), Object::Int(8)),
            (Name::from("ColorSpace"), Object::Name(Name::from(space))),
            (Name::from("Filter"), Object::Name(Name::from("DCTDecode"))),
        ];
        if let Some(values) = decode {
            pairs.push((
                Name::from("Decode"),
                Object::Array(values.into_iter().map(Object::Real).collect()),
            ));
        }
        let mut diags = Diagnostics::default();
        super::ImageDict::load(&Dict::from_pairs(pairs), &NoResolve, &mut diags)
            .expect("the fixture dictionary should load")
    }

    #[test]
    fn a_decode_array_reaches_a_codecs_output_too() {
        // `TranslateScanline24bpp` runs on the decoder's scanline, so the
        // Adobe inversion a CMYK JPEG carries as `/Decode [1 0 …]` has to be
        // applied to the codec's bytes — `bug_1646` and `bug_718762`.
        let info = codec_dict(
            "DeviceCMYK",
            Some(vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]),
        );
        assert!(!info.default_decode);
        let space = crate::color::ColorSpace::DeviceCmyk;
        let mut data = vec![255u8, 0, 0, 253, 0, 255, 255, 2];
        super::apply_codec_decode(&mut data, Some(&space), 4, &info);
        assert_eq!(data, vec![0u8, 255, 255, 2, 255, 0, 0, 253]);
    }

    #[test]
    fn the_default_decode_leaves_a_codecs_output_untouched() {
        // The default mapping is the identity by construction, so it is
        // skipped rather than run — no rounding drift on the common path.
        let info = codec_dict("DeviceCMYK", None);
        assert!(info.default_decode);
        let space = crate::color::ColorSpace::DeviceCmyk;
        let original = vec![255u8, 0, 0, 253, 1, 2, 3, 4];
        let mut data = original.clone();
        super::apply_codec_decode(&mut data, Some(&space), 4, &info);
        assert_eq!(data, original);
        // An explicit array that *equals* the default is skipped as well.
        let info = codec_dict(
            "DeviceCMYK",
            Some(vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]),
        );
        let mut data = original.clone();
        super::apply_codec_decode(&mut data, Some(&space), 4, &info);
        assert_eq!(data, original);
    }

    /// The row conversion is the colour space's own answer, over every input.
    ///
    /// [`super::rows::Converted`] replaced a per-pixel `sample_bytes`, which
    /// had in turn replaced a float round trip through [`Rgb`]. Both
    /// replacements were justified by being *exactly* the thing they replaced,
    /// so the claim is checked over every input each variant can take rather
    /// than over a sample of them — the float path is written out here as the
    /// reference, because it is the one nobody would accuse of being an
    /// optimisation.
    #[test]
    fn the_row_conversion_is_exactly_the_float_path() {
        // The round trip the whole argument rests on, both directions, all
        // 256: a byte through `f32 / 255.0` and back through
        // `(v.clamp(0, 1) * 255).round()` is the identity, so a conversion
        // that skips the floats cannot differ from one that does not.
        for b in 0..=255u8 {
            let there = f32::from(b) / 255.0;
            let back = Rgb {
                r: there,
                g: there,
                b: there,
            }
            .to_bytes();
            assert_eq!(back, [b, b, b], "byte {b} does not survive the float trip");
        }

        // Grey: every byte, widened to three equal channels.
        let gray = Samples::Whole(Pixels::Gray8((0..=255u8).collect()));
        for x in 0..256u32 {
            let v = u8::try_from(x).expect("x < 256");
            assert_eq!(sample_at(&gray, x, 0, 256), [v, v, v], "gray {x}");
        }

        // RGB: a walk that puts every byte in every channel position.
        let rgb = Samples::Whole(Pixels::Rgb8(
            (0..=255u8).flat_map(|v| [v, 255 - v, v / 2]).collect(),
        ));
        for x in 0..256u32 {
            let v = u8::try_from(x).expect("x < 256");
            assert_eq!(sample_at(&rgb, x, 0, 256), [v, 255 - v, v / 2], "rgb {x}");
        }

        // CMYK is the one with real arithmetic in it. The full domain is 2^32,
        // so this walks a lattice that hits every value on every axis, plus the
        // saturated corners the interpolation treats specially.
        let mut cmyk = Vec::new();
        let step = 17u16; // 0, 17, ... 255 — sixteen values, exact at both ends.
        for c in (0..=255u16).step_by(step as usize) {
            for m in (0..=255u16).step_by(step as usize) {
                for y in (0..=255u16).step_by(step as usize) {
                    for k in (0..=255u16).step_by(step as usize) {
                        cmyk.extend_from_slice(&[c as u8, m as u8, y as u8, k as u8]);
                    }
                }
            }
        }
        let count = cmyk.len() / 4;
        let raw = cmyk.clone();
        let cmyk = Samples::Whole(Pixels::Cmyk8(cmyk.into()));
        // The whole lattice is one row, converted once — which is how the
        // pipeline is meant to be used. Calling `sample_at` per point would
        // rebuild the row buffer for each of the 65 536 of them.
        let row = converted_row(&cmyk, count as u32, 0);
        for (x, got) in row.iter().enumerate() {
            let at = x * 4;
            let want = crate::color::ColorSpace::DeviceCmyk
                .to_rgb(&[
                    f32::from(raw[at]) / 255.0,
                    f32::from(raw[at + 1]) / 255.0,
                    f32::from(raw[at + 2]) / 255.0,
                    f32::from(raw[at + 3]) / 255.0,
                ])
                .to_bytes();
            assert_eq!(*got, want, "cmyk lattice point {x}");
        }

        // Indexed, whose palette the row pipeline encodes once rather than per
        // pixel — the same answer, which is the whole point of `Palette`.
        let palette: Box<[Rgb]> = (0..=255u8)
            .map(|v| Rgb {
                r: f32::from(v) / 255.0,
                g: f32::from(255 - v) / 255.0,
                b: 0.25,
            })
            .collect();
        let indexed = Samples::Whole(Pixels::Indexed {
            indices: (0..=255u8).collect(),
            palette: palette.clone(),
        });
        for x in 0..256u32 {
            let want = palette[x as usize].to_bytes();
            assert_eq!(sample_at(&indexed, x, 0, 256), want, "indexed {x}");
        }

        // A stencil, both phases: a set bit is ink and reads 0, a clear one
        // reads 255. The stencil's own colour is applied later, by the render
        // path, so the conversion only has to preserve that convention.
        let bits = BitImage {
            width: 2,
            height: 1,
            row_bytes: 1,
            bits: vec![0b1000_0000],
        };
        let stencil = Samples::Whole(Pixels::Stencil(bits));
        assert_eq!(sample_at(&stencil, 0, 0, 2), [0, 0, 0], "a set bit is ink");
        assert_eq!(
            sample_at(&stencil, 1, 0, 2),
            [255, 255, 255],
            "a clear bit is paper"
        );

        // And an out-of-range read is black on every variant rather than a
        // panic, which is the fallback the per-pixel path carried.
        for p in [&gray, &rgb, &cmyk, &indexed, &stencil] {
            assert_eq!(
                sample_at(p, 9999, 9999, 256),
                [0, 0, 0],
                "an out-of-range read is black"
            );
        }
    }

    /// [`super::mask_plane`]'s grey fast arm must be its general arm, which
    /// is the row pipeline, which is in turn the float path by the test
    /// above. Asserted here rather than argued in the comment, because the
    /// conformance gate can only see the mask shapes the corpus happens to
    /// carry and an `Indexed` `/SMask` is not one of them.
    #[test]
    fn the_mask_planes_fast_arms_are_the_general_one() {
        let general = |pixels: &Samples, w: u32, h: u32| -> Vec<u8> {
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| sample_at(pixels, x, y, w)[0])
                .collect()
        };

        // Grey, at the exact length, short, and long.
        for (w, h, len) in [(4_u32, 3_u32, 12_usize), (4, 3, 7), (4, 3, 20), (1, 1, 1)] {
            let data: Box<[u8]> = (0..len).map(|i| (i * 31 % 256) as u8).collect();
            let pixels = Samples::Whole(Pixels::Gray8(data));
            let got = super::mask_plane(&pixels, w, h).expect("dimensions multiply");
            let mut want = general(&pixels, w, h);
            want.resize((w * h) as usize, 0);
            assert_eq!(&got[..], &want[..], "gray {w}x{h}, {len} bytes");
        }

        // Indexed, whose palette the fast arm encodes once.
        let palette: Box<[Rgb]> = (0..=255u8)
            .map(|v| Rgb {
                r: f32::from(v) / 255.0,
                g: 0.5,
                b: 0.25,
            })
            .collect();
        for (w, h, len) in [(8_u32, 4_u32, 32_usize), (8, 4, 10)] {
            let pixels = Samples::Whole(Pixels::Indexed {
                indices: (0..len).map(|i| (i * 7 % 256) as u8).collect(),
                palette: palette.clone(),
            });
            let got = super::mask_plane(&pixels, w, h).expect("dimensions multiply");
            let mut want = general(&pixels, w, h);
            want.resize((w * h) as usize, 0);
            assert_eq!(&got[..], &want[..], "indexed {w}x{h}, {len} indices");
        }

        // And the general arm still answers for the kinds that have no fast
        // one, so a mask that is not grey is not silently dropped.
        let rgb = Samples::Whole(Pixels::Rgb8((0..24u8).collect()));
        let got = super::mask_plane(&rgb, 4, 2).expect("dimensions multiply");
        assert_eq!(&got[..], &general(&rgb, 4, 2)[..]);
    }

    /// The product cap, not each axis: 65536 square sits inside the
    /// dictionary gate and is 4.3 Gpx.
    #[test]
    fn a_gigapixel_image_is_not_a_workable_area() {
        assert!(
            super::image_area_is_workable(20_000, 28_000),
            "A0 at 600dpi"
        );
        assert!(!super::image_area_is_workable(65_536, 65_536), "4.3 Gpx");
        assert!(!super::image_area_is_workable(131_071, 131_071), "17 Gpx");
        assert!(super::image_area_is_workable(2, 2));
    }

    /// A mask plane past the area cap is dropped, it does not abort or hang.
    ///
    /// `mask_plane`'s buffer is the mask's own `/Width` x `/Height` at one
    /// byte a pixel, and the dictionary gate accepts each axis up to 131071 —
    /// a 17 GB `Vec` reached from a base image of any size, because a mask is
    /// never resolution-reduced. `try_reserve_exact` is not a reliable refusal
    /// for that: Linux overcommit lets the reserve succeed, and zeroing the
    /// buffer then OOMs the host. [`super::image_area_is_workable`] is what
    /// turns it into `None` without touching the allocator.
    #[test]
    fn a_mask_plane_too_large_to_allocate_is_dropped_rather_than_aborting() {
        let pixels = Samples::Whole(Pixels::Gray8(Box::new([0u8; 4])));
        assert_eq!(super::mask_plane(&pixels, 131_071, 131_071), None);
        // 65536 square is inside each axis cap and is 4.3 Gpx — the hang the
        // render path already refuses, and the size that would still pass a
        // reserve-only check on an overcommit host.
        assert_eq!(super::mask_plane(&pixels, 65_536, 65_536), None);
        // The same samples at a size that fits still produce a plane.
        assert!(super::mask_plane(&pixels, 2, 2).is_some());
    }

    #[test]
    fn the_decode_table_is_exhaustively_the_per_sample_arithmetic() {
        // The table replaces a loop that ran the float mapping on every byte of
        // the image. It is only allowed to be faster, never different — so this
        // asserts the equality on *every* input the mapping can receive:
        // each of the four component slots, each of the 256 byte values, over
        // several arrays that reach different corners of the arithmetic
        // (the Adobe inversion, an asymmetric range, one that clamps at both
        // ends, and a degenerate zero-width range).
        let arrays: [Vec<f32>; 4] = [
            vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.5, 0.25, 1.0, 0.1, 0.9, 0.0, 1.0],
            vec![-1.0, 2.0, 2.0, -1.0, -0.5, 1.5, 1.5, -0.5],
            vec![0.3, 0.3, 0.0, 1.0, 1.0, 0.0, 0.7, 0.2],
        ];
        let space = crate::color::ColorSpace::DeviceCmyk;
        for values in arrays {
            let info = codec_dict("DeviceCMYK", Some(values));
            let decode = super::DecodeMap::new(Some(&space), 4, 8, info.decode.as_ref());
            let table = super::decode_table(&decode, 4);
            for (component, row) in table.iter().enumerate() {
                for raw in 0..=255u8 {
                    let value = decode.apply(component, f32::from(raw));
                    let expected = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                    assert_eq!(
                        row[usize::from(raw)],
                        expected,
                        "component {component}, raw {raw}"
                    );
                }
            }
            assert_eq!(table.len(), 4, "one row per component");
        }
    }

    #[test]
    fn a_trailing_partial_pixel_maps_by_its_position_in_the_pixel() {
        // The per-sample loop keyed on `i % components`, so a buffer whose
        // length is not a whole number of pixels still mapped its last bytes as
        // components 0, 1, … The chunked loop has to agree, which it does only
        // because `chunks_mut` yields the short trailing chunk rather than
        // dropping it. Four components, six bytes: the last two are components
        // 0 and 1 of a pixel that is not all there.
        let info = codec_dict(
            "DeviceCMYK",
            Some(vec![1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0]),
        );
        let space = crate::color::ColorSpace::DeviceCmyk;
        let mut data = vec![10u8, 20, 30, 40, 50, 60];
        super::apply_codec_decode(&mut data, Some(&space), 4, &info);
        // Components 0 and 2 invert; 1 and 3 are the identity.
        assert_eq!(data, vec![245u8, 20, 225, 40, 205, 60]);
    }

    #[test]
    fn a_codec_decode_needs_a_space_and_some_components() {
        let info = codec_dict("DeviceGray", Some(vec![1.0, 0.0]));
        let original = vec![10u8, 200];
        // No colour space, or no components, and nothing happens.
        let mut data = original.clone();
        super::apply_codec_decode(&mut data, None, 1, &info);
        assert_eq!(data, original);
        let mut data = original.clone();
        let gray = crate::color::ColorSpace::DeviceGray;
        super::apply_codec_decode(&mut data, Some(&gray), 0, &info);
        assert_eq!(data, original);
        // With both, grey inverts.
        let mut data = original.clone();
        super::apply_codec_decode(&mut data, Some(&gray), 1, &info);
        assert_eq!(data, vec![245u8, 55]);
    }

    /// A Group 4 stream of `rows` all-white rows.
    ///
    /// One vertical-zero code — a single set bit — carries a row whose first
    /// changing element is the row's end, which against an all-white reference
    /// line is an all-white row.
    fn all_white_g4(rows: usize) -> Vec<u8> {
        let mut byte = 0u8;
        for i in 0..rows.min(8) {
            byte |= 1 << (7 - i);
        }
        vec![byte]
    }

    /// A Group 4 stream whose first row is eight black pixels then white, and
    /// whose remaining rows repeat it.
    ///
    /// Horizontal mode (`001`) with a zero-length white run (`00110101`) and an
    /// eight-long black run (`000101`); each further row is a vertical-zero
    /// code (`1`), which copies the row above.
    fn black_then_white_g4(rows: usize) -> Vec<u8> {
        let mut bits = String::from("001001101010001011");
        for _ in 1..rows {
            bits.push('1');
        }
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        bits.as_bytes()
            .chunks(8)
            .filter_map(|c| {
                let s = std::str::from_utf8(c).ok()?;
                u8::from_str_radix(s, 2).ok()
            })
            .collect()
    }

    fn ccitt_stream(width: i64, height: i64, mask: bool, data: &[u8]) -> Stream {
        let parms = Dict::from_pairs(vec![
            (Name::from("K"), Object::Int(-1)),
            (Name::from("Columns"), Object::Int(width)),
            (Name::from("Rows"), Object::Int(height)),
        ]);
        let mut pairs = vec![
            (Name::from("Width"), Object::Int(width)),
            (Name::from("Height"), Object::Int(height)),
            (Name::from("BitsPerComponent"), Object::Int(1)),
            (
                Name::from("Filter"),
                Object::Name(Name::from("CCITTFaxDecode")),
            ),
            (Name::from("DecodeParms"), Object::Dict(parms)),
        ];
        if mask {
            pairs.push((Name::from("ImageMask"), Object::Bool(true)));
        } else {
            pairs.push((
                Name::from("ColorSpace"),
                Object::Name(Name::from("DeviceGray")),
            ));
        }
        stream(pairs, data)
    }

    #[test]
    fn a_fax_image_reaches_the_decoder_at_all() {
        // The chain classifies `/CCITTFaxDecode` as an image codec and hands
        // its bytes back undecoded, so without a caller in the image path the
        // codestream was unpacked as if it were already samples. An all-white
        // image is the smallest thing that tells the two apart: decoded it is
        // white, undecoded the single data byte `0xE0` paints three black
        // pixels across the top row.
        let s = ccitt_stream(20, 3, false, &all_white_g4(3));
        let image = decode(&s).expect("should decode");
        assert_eq!((image.width, image.height), (20, 3));
        for y in 0..3 {
            for x in 0..20 {
                assert_eq!(
                    sample_at(&image.samples, x, y, 20),
                    [255, 255, 255],
                    "({x},{y}) should be white"
                );
            }
        }
    }

    #[test]
    fn a_fax_row_is_repacked_from_four_byte_padding_to_the_images_pitch() {
        // The decoder pads a row to four bytes; every consumer here reads rows
        // at `width.div_ceil(8)`. At width 20 those are 4 and 3, so reading the
        // decoder's buffer at the image's pitch would start each row a byte
        // further into the previous one and shear the picture. Three rows of a
        // 20-wide image is 9 sample bytes, not 12.
        //
        // The pattern has to be non-uniform for the shear to show. Rows 0 and 1
        // are eight black pixels then white; row 2 is the decoder's white
        // prefill, because the byte padding ends the codestream before it.
        // Read at the decoder's stride instead of the image's, row 1's black
        // byte would land four bytes on — a third of the way into row 1's
        // pixels rather than at its start.
        let s = ccitt_stream(20, 3, false, &black_then_white_g4(3));
        let image = decode(&s).expect("should decode");
        let black = [0_u8, 0, 0];
        let white = [255_u8, 255, 255];
        for y in 0..3 {
            for x in 0..20 {
                let want = if y < 2 && x < 8 { black } else { white };
                assert_eq!(
                    sample_at(&image.samples, x, y, 20),
                    want,
                    "({x},{y}) — a shear puts the black run somewhere else"
                );
            }
        }
    }

    #[test]
    fn a_fax_stream_that_will_not_decode_leaves_the_image_white() {
        // The decoder pre-fills white and gives back the rows it managed; the
        // repack keeps that, so damage is blank rather than black or an error.
        let s = ccitt_stream(20, 3, false, &[0x00, 0x00]);
        let image = decode(&s).expect("damage is not a failure");
        assert_eq!(sample_at(&image.samples, 0, 0, 20), [255, 255, 255]);
    }

    /// Build a `[/Separation /Name /DeviceCMYK <tint transform>]` array whose
    /// transform is the type-2 exponential `C0 -> C1` at `N = 1`.
    fn separation_cmyk(c1: [f32; 4]) -> Object {
        let mut c0 = Array::default();
        for _ in 0..4 {
            c0.push(Object::Real(0.0));
        }
        let mut c1_arr = Array::default();
        for v in c1 {
            c1_arr.push(Object::Real(v));
        }
        let mut domain = Array::default();
        domain.push(Object::Int(0));
        domain.push(Object::Int(1));
        let mut range = Array::default();
        for _ in 0..4 {
            range.push(Object::Int(0));
            range.push(Object::Int(1));
        }
        let tint = Dict::from_pairs(vec![
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("N"), Object::Real(1.0)),
            (Name::from("Domain"), Object::Array(domain)),
            (Name::from("Range"), Object::Array(range)),
            (Name::from("C0"), Object::Array(c0)),
            (Name::from("C1"), Object::Array(c1_arr)),
        ]);
        let mut space = Array::default();
        space.push(Object::Name(Name::from("Separation")));
        space.push(Object::Name(Name::from("Spot")));
        space.push(Object::Name(Name::from("DeviceCMYK")));
        space.push(Object::Dict(tint));
        Object::Array(space)
    }

    #[test]
    fn a_separation_image_runs_its_samples_through_the_tint_transform() {
        // `corpus/fx/other/1.pdf`'s own image, reduced to its essentials: one
        // eight-bit sample of `0xC6` in a `/Separation` whose alternate is
        // `DeviceCMYK` and whose `C1` is PANTONE 327 CV. The tint is
        // 198/255 = 0.7765, so the CMYK is (0.7765, 0, 0.4659, 0) and the
        // Adobe table turns that into teal.
        //
        // Read as a grey level instead — which is what a `Pixels::Gray8`
        // bucketed on the component count does — the same byte paints
        // (198, 198, 198). That was the defect: the sample is a *tint*, and
        // only the tint transform makes it a colour.
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(1)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    separation_cmyk([1.0, 0.0, 0.600_006, 0.0]),
                ),
            ],
            &[0xC6],
        );
        let image = decode(&s).expect("should decode");
        assert_eq!(
            sample_at(&image.samples, 0, 0, 1),
            [0, 182, 162],
            "the tint must reach the alternate space, not the page as grey"
        );
        // The shape matters as much as the colour: a palette is what the
        // renderer's indexed fast path consumes, and it must span the whole
        // eight-bit sample domain rather than only the values in use.
        let Samples::Whole(Pixels::Indexed { palette, .. }) = &image.samples else {
            panic!(
                "a resolved Separation image is a palette, got {:?}",
                image.samples
            );
        };
        assert_eq!(palette.len(), 256);
        // A zero tint is `C0`, which is CMYK all-zero — paper white.
        assert_eq!(palette[0].to_bytes(), [255, 255, 255]);
    }

    #[test]
    fn a_separation_decode_array_is_folded_into_the_palette() {
        // `/Decode [1 0]` inverts the *tint* before the transform runs, so
        // the `0xC6` sample becomes a 1 - 0.7765 = 0.2235 tint rather than a
        // 0.7765 one. `LoadPalette` folds `decode_min_ + decode_step_ * i`
        // into each entry, and so must this.
        let mut decode_arr = Array::default();
        decode_arr.push(Object::Int(1));
        decode_arr.push(Object::Int(0));
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(1)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    separation_cmyk([1.0, 0.0, 0.600_006, 0.0]),
                ),
                (Name::from("Decode"), Object::Array(decode_arr)),
            ],
            &[0xC6],
        );
        let image = decode(&s).expect("should decode");
        let inverted = sample_at(&image.samples, 0, 0, 1);
        let s_plain = stream(
            vec![
                (Name::from("Width"), Object::Int(1)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (
                    Name::from("ColorSpace"),
                    separation_cmyk([1.0, 0.0, 0.600_006, 0.0]),
                ),
            ],
            &[255 - 0xC6],
        );
        let plain = decode(&s_plain).expect("should decode");
        assert_eq!(
            inverted,
            sample_at(&plain.samples, 0, 0, 1),
            "`/Decode [1 0]` on a tint is the complement of the sample"
        );
    }

    #[test]
    fn a_devicen_image_converts_per_pixel_rather_than_through_a_palette() {
        // Two colorants cannot be tabulated over an eight-bit sample, so this
        // takes `TranslateScanline24bpp`'s per-pixel shape and resolves to
        // `Rgb8`. The transform is a type-2 exponential from all-zero to
        // `C1`, evaluated on the *first* input only, which is what a
        // one-output-per-colorant `DeviceN` degenerates to here — the point of
        // the fixture is the arm taken, and that both colorants reach it.
        let mut names = Array::default();
        names.push(Object::Name(Name::from("SpotA")));
        names.push(Object::Name(Name::from("SpotB")));
        let mut domain = Array::default();
        for _ in 0..2 {
            domain.push(Object::Int(0));
            domain.push(Object::Int(1));
        }
        let mut range = Array::default();
        for _ in 0..4 {
            range.push(Object::Int(0));
            range.push(Object::Int(1));
        }
        let mut c0 = Array::default();
        let mut c1 = Array::default();
        for _ in 0..4 {
            c0.push(Object::Real(0.0));
        }
        for v in [0.0_f32, 1.0, 1.0, 0.0] {
            c1.push(Object::Real(v));
        }
        let tint = Dict::from_pairs(vec![
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("N"), Object::Real(1.0)),
            (Name::from("Domain"), Object::Array(domain)),
            (Name::from("Range"), Object::Array(range)),
            (Name::from("C0"), Object::Array(c0)),
            (Name::from("C1"), Object::Array(c1)),
        ]);
        let mut space = Array::default();
        space.push(Object::Name(Name::from("DeviceN")));
        space.push(Object::Array(names));
        space.push(Object::Name(Name::from("DeviceCMYK")));
        space.push(Object::Dict(tint));
        let s = stream(
            vec![
                (Name::from("Width"), Object::Int(2)),
                (Name::from("Height"), Object::Int(1)),
                (Name::from("BitsPerComponent"), Object::Int(8)),
                (Name::from("ColorSpace"), Object::Array(space)),
            ],
            &[0x00, 0x00, 0xFF, 0xFF],
        );
        let image = decode(&s).expect("should decode");
        assert!(
            matches!(image.samples, Samples::Whole(Pixels::Rgb8(_))),
            "a two-colorant DeviceN resolves per pixel and cannot stay packed, \
             got {:?}",
            image.samples
        );
        // A zero tint vector is `C0` — CMYK all-zero, paper white — and a
        // full one is `C1`, pure red in the alternate. Neither is the raw
        // sample pair, which is the whole point.
        assert_eq!(sample_at(&image.samples, 0, 0, 2), [255, 255, 255]);
        let full = sample_at(&image.samples, 1, 0, 2);
        assert!(
            full[0] > 200 && full[1] < 80 && full[2] < 80,
            "a full tint must reach the alternate space's red, got {full:?}"
        );
    }
}
