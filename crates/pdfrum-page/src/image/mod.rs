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
//! SPEC.md §7 pins an owned [`ImageData`] instead (design brief D22): every
//! per-scanline *behaviour* is preserved — the truncated-stream zero pad, the
//! palette packing, the sixteen-bit high-byte truncation — and only the
//! laziness is gone. Memory is bounded by the same four-gibibyte cap the C++
//! enforces.

mod cache;
mod dct;
mod decode_array;
pub mod dict;
mod jbig2;
mod jpx;
mod mask;
mod scanline;

pub use cache::{ImageCache, MAX_BYTES, MAX_ENTRIES, RequestedSize};
pub use dct::{
    ADOBE_CMYK_DECODE, DctImage, allows_reduced_resolution, decode_dct, probe as probe_dct,
    scale_denominator, scaled_size,
};
pub use decode_array::DecodeMap;
pub use dict::{ImageDict, MAX_DIMENSION};
pub use jbig2::{BitImage, decode_jbig2};
pub use jpx::{JpxAction, JpxColorSpace, JpxImage, decode_jpx, is_stock_device};
pub use mask::{ColorKey, ImageMask, matte_color};
pub use scanline::{
    get_bits, invert_line, palette_index, rgb_line_to_bgr, scale_to_byte, scanline,
};

use crate::color::{ColorSpace, Rgb};
use crate::error::Error;
use crate::function::FunctionCache;
use crate::names;
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_filters::{CcittParams, Filter, decode_ccitt, decode_chain};
use pdfrum_object::{Dict, Object, Resolve, Stream};

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

    /// The colour at `(x, y)` in an image `width` samples across.
    #[must_use]
    pub fn color_at(&self, x: u32, y: u32, width: u32) -> Rgb {
        let Some(index) = usize::try_from(y)
            .ok()
            .and_then(|row| row.checked_mul(usize::try_from(width).ok()?))
            .and_then(|base| base.checked_add(usize::try_from(x).ok()?))
        else {
            return Rgb::BLACK;
        };
        let byte = |i: usize, data: &[u8]| f32::from(data.get(i).copied().unwrap_or(0)) / 255.0;
        match self {
            Self::Stencil(b) => {
                let v = if b.pixel(x, y) { 0.0 } else { 1.0 };
                Rgb { r: v, g: v, b: v }
            }
            Self::Gray8(d) => {
                let v = byte(index, d);
                Rgb { r: v, g: v, b: v }
            }
            Self::Rgb8(d) => Rgb {
                r: byte(index * 3, d),
                g: byte(index * 3 + 1, d),
                b: byte(index * 3 + 2, d),
            },
            Self::Cmyk8(d) => crate::color::ColorSpace::DeviceCmyk.to_rgb(&[
                byte(index * 4, d),
                byte(index * 4 + 1, d),
                byte(index * 4 + 2, d),
                byte(index * 4 + 3, d),
            ]),
            Self::Indexed { indices, palette } => {
                let i = indices.get(index).copied().unwrap_or(0);
                palette.get(usize::from(i)).copied().unwrap_or(Rgb::BLACK)
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
    /// The pixels.
    pub pixels: Pixels,
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
        self.pixels.byte_size()
            + match &self.mask {
                Some(ImageMask::Alpha { alpha, .. }) => alpha.len(),
                _ => 0,
            }
    }
}

/// Decode an image `XObject`.
///
/// `form_resources` is consulted for a named colour space **only for inline
/// images**; a real image `XObject` sees the page's resources alone. `size`
/// says how much resolution the caller needs, which only the DCT and JPEG
/// 2000 codecs act on.
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

    // The codecs, dispatched on the last filter.
    let (width, height, pixels, jpx_alpha) = match info.last_filter {
        Some(Filter::Jpx) => {
            let levels = size.levels(info.width, info.height);
            let smask_in_data = stream.dict.int(names::SMASK_IN_DATA, r).unwrap_or(0);
            let image = decode_jpx(&decoded.data, space.as_ref(), smask_in_data, levels, limits)
                .inspect_err(|_| {
                    diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
                })?;
            if image.space_override.is_some() {
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
            (image.width, image.height, pixels, image.alpha)
        }
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
                (info.width, info.height, Pixels::Stencil(bits), None)
            } else {
                let mut samples = bits.bits;
                for byte in &mut samples {
                    *byte = !*byte;
                }
                let pixels = unpack(&info, space.as_ref(), &samples, diags)?;
                (info.width, info.height, pixels, None)
            }
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
            (image.width, image.height, pixels, None)
        }
        // A one-bit fax image with a colour space of its own: the bits are the
        // samples, so they go through `unpack` like any other 1-bit picture and
        // pick up `/Decode` and the palette on the way.
        Some(Filter::CcittFax) => {
            let samples = ccitt_samples(&info, &decoded.data, r, diags)?;
            let pixels = unpack(&info, space.as_ref(), &samples, diags)?;
            (info.width, info.height, pixels, None)
        }
        _ => {
            if decoded.image.is_some() && info.last_filter.is_none() {
                return Err(Error::ImageUndecodable {
                    what: "an unrecognised filter left no decoder",
                });
            }
            let pixels = unpack(&info, space.as_ref(), &decoded.data, diags)?;
            (info.width, info.height, pixels, None)
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
        pixels,
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
/// samples are gone. PDFium runs it inside `CPDF_DIB::GetScanline`, filling a
/// parallel BGRA buffer whose alpha byte is
///
/// ```text
/// dest.alpha = IsColorIndexOutOfBounds(index, comp_data_[0]) ? 0xFF : 0;
/// ```
///
/// — **in range means transparent**, which reads backwards until you notice
/// the predicate is named for the opposite. `bug_343075986.in` masks index 0
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
/// This is the precondition of `CPDF_DIB::GetScanline`'s zeroed-output arm,
/// and it is easy to get wrong because the arm itself does not name it. The
/// C++ picks a source in this order:
///
/// ```text
/// if (cached_bitmap_ && ...)          src_line = cached_bitmap_->GetScanline(line);
/// else if (decoder_)                  src_line = decoder_->GetScanline(line);
/// else if (GetSize() > line * pitch)  ... zero-pad what remains ...
/// if (src_line.empty()) { fill(result, 0); return result; }   // decode skipped
/// ```
///
/// A codec — JBIG2, JPX, DCT, CCITT, or a Flate/RunLength predictor built as
/// a scanline decoder — always hands back a full row, so the empty case never
/// arises and every row is decoded normally. Only a stream read directly can
/// run out. Applying the zeroed arm to a codec's output instead makes a
/// truncated JBIG2 mask stop inverting partway down, which is what
/// `bug_674771.in` showed.
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
        Some(ccitt_samples(info, &decoded.data, r, diags)?)
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
        pixels: Pixels::Stencil(BitImage {
            width: info.width,
            height: info.height,
            row_bytes,
            bits,
        }),
        mask: None,
        matte: None,
        interpolate: stream.dict.bool(names::INTERPOLATE).unwrap_or(false),
    })
}

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
    diags: &mut Diagnostics,
) -> Result<Vec<u8>, Error> {
    let params = CcittParams::from_dict(&info.params, r);
    let image = decode_ccitt(data, params, info.width, info.height, diags).map_err(|_| {
        diags.record(Severity::Suspicious, DiagKind::ImageDecodeFailed, None);
        Error::ImageUndecodable {
            what: "CCITT fax data would not decode",
        }
    })?;
    let pitch = info.pitch().ok_or(Error::ImageTooLarge)?;
    let total = info.total_bytes().ok_or(Error::ImageTooLarge)?;
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
        pixels: Pixels::Stencil(image),
        mask: None,
        matte: None,
        interpolate: stream.dict.bool(names::INTERPOLATE).unwrap_or(false),
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

/// Unpack raw or losslessly-filtered samples into pixels.
fn unpack(
    info: &ImageDict,
    space: Option<&ColorSpace>,
    data: &[u8],
    diags: &mut Diagnostics,
) -> Result<Pixels, Error> {
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

    // An indexed image keeps its indices and a resolved palette, which is
    // what a renderer needs to resample it without blending indices.
    if let ColorSpace::Indexed(indexed) = space {
        let mut indices = vec![0u8; total_pixels];
        let mut padded = false;
        for y in 0..rows {
            let (line, availability) =
                scanline::scanline(data, u32::try_from(y).unwrap_or(0), pitch);
            padded |= availability != scanline::Availability::Whole;
            // An absent row returns a zeroed *output* buffer without ever
            // reaching the decode, so the indices stay zero whatever `/Decode`
            // maps a zero sample to. See `Availability`.
            if availability == scanline::Availability::Absent {
                continue;
            }
            for x in 0..pixels_per_row {
                let raw = scanline::get_bits(&line, x * info.bpc as usize, info.bpc);
                // An `/Decode` on an indexed image remaps the index itself.
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the clamp bounds the value to a palette index"
                )]
                let mapped = decode.apply(0, f64_to_f32(raw)).clamp(0.0, 255.0) as u8;
                if let Some(slot) = indices.get_mut(y * pixels_per_row + x) {
                    *slot = mapped;
                }
            }
        }
        if padded {
            diags.record(Severity::Recovered, DiagKind::ImageStreamTruncated, None);
        }
        let palette = (0..=indexed.max_index)
            .map(|i| space.to_rgb(&[f32::from(i)]))
            .collect();
        return Ok(Pixels::Indexed {
            indices: indices.into(),
            palette,
        });
    }

    // Everything else widens to eight bits per component in the space's own
    // component order.
    let mut out = vec![
        0u8;
        total_pixels
            .checked_mul(components)
            .ok_or(Error::ImageTooLarge)?
    ];
    let mut padded = false;
    for y in 0..rows {
        let (line, availability) = scanline::scanline(data, u32::try_from(y).unwrap_or(0), pitch);
        padded |= availability != scanline::Availability::Whole;
        // An absent row never reaches `TranslateScanline24bpp`: PDFium returns
        // a zeroed *output* buffer, so the pixels are literal black rather
        // than whatever `/Decode` maps a zero sample to. See `Availability`.
        if availability == scanline::Availability::Absent {
            continue;
        }
        for x in 0..pixels_per_row {
            for c in 0..components {
                let bit_pos = (x * components + c) * info.bpc as usize;
                let raw = scanline::get_bits(&line, bit_pos, info.bpc);
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "raw samples cap at 16 bits, exact in f32"
                )]
                let value = decode.apply(c, raw as f32);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the clamp bounds the product to 0..=255"
                )]
                let byte = (value.clamp(0.0, 1.0) * 255.0) as u8;
                if let Some(slot) = out.get_mut((y * pixels_per_row + x) * components + c) {
                    *slot = byte;
                }
            }
        }
    }
    if padded {
        diags.record(Severity::Recovered, DiagKind::ImageStreamTruncated, None);
    }
    let _ = max_raw;
    Ok(match components {
        1 => Pixels::Gray8(out.into()),
        4 => Pixels::Cmyk8(out.into()),
        _ => Pixels::Rgb8(out.into()),
    })
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
    for (i, sample) in data.iter_mut().enumerate() {
        let value = decode.apply(i % components, f32::from(*sample));
        // **Rounding**, not the truncation the raw-sample path uses. PDFium
        // carries these values as floats all the way into the colour
        // conversion and only truncates the *converted* byte; we have to land
        // them back in a byte here, so the encode has to be the one that makes
        // the round trip exact. Truncating instead loses a count on the
        // commonest case of all — the `[1 0]` inversion, where `1 - 253/255`
        // lands a hair under `2/255` and would come back as 1.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the clamp bounds the product to 0..=255"
        )]
        let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        *sample = byte;
    }
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
    let pixels = usize::try_from(image.width)
        .ok()?
        .checked_mul(usize::try_from(image.height).ok()?)?;
    let mut alpha = vec![0u8; pixels];
    for y in 0..image.height {
        for x in 0..image.width {
            let rgb = image.pixels.color_at(x, y, image.width);
            let Some(index) = usize::try_from(y)
                .ok()
                .and_then(|row| row.checked_mul(usize::try_from(image.width).ok()?))
                .and_then(|base| base.checked_add(usize::try_from(x).ok()?))
            else {
                continue;
            };
            if let Some(slot) = alpha.get_mut(index) {
                // A soft mask's alpha is its luminosity; a stencil's is its
                // coverage.
                *slot = rgb.to_bytes()[0];
            }
        }
    }
    Some(ImageMask::Alpha {
        width: image.width,
        height: image.height,
        alpha: alpha.into(),
        stencil,
    })
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

    use super::{ImageData, Pixels, RequestedSize, decode_image};
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
            image.pixels,
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
            image.pixels,
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
        let Pixels::Gray8(gray) = &image.pixels else {
            panic!("expected grey samples, got {:?}", image.pixels);
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
        let Pixels::Stencil(BitImage {
            bits, row_bytes, ..
        }) = &image.pixels
        else {
            panic!("expected a stencil, got {:?}", image.pixels);
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
        let Pixels::Stencil(BitImage { bits, .. }) = &image.pixels else {
            panic!("expected a stencil, got {:?}", image.pixels);
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
        let Pixels::Stencil(BitImage { bits, .. }) = &image.pixels else {
            panic!("expected a stencil, got {:?}", image.pixels);
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
        let Pixels::Stencil(BitImage { bits, .. }) = &image.pixels else {
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
            image.pixels,
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
        let Pixels::Indexed { indices, palette } = &image.pixels else {
            panic!("expected indexed pixels, got {:?}", image.pixels);
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
        let pixels = Pixels::Rgb8(Box::from(&[255u8, 0, 0, 0, 255, 0][..]));
        let red = pixels.color_at(0, 0, 2);
        assert!((red.r - 1.0).abs() < 1e-6);
        // Out of range reads as black rather than panicking.
        assert_eq!(pixels.color_at(99, 99, 2), Rgb::BLACK);
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
                    image.pixels.color_at(x, y, 20),
                    Rgb {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0
                    },
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
        let black = Rgb {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        };
        let white = Rgb {
            r: 1.0,
            g: 1.0,
            b: 1.0,
        };
        for y in 0..3 {
            for x in 0..20 {
                let want = if y < 2 && x < 8 { black } else { white };
                assert_eq!(
                    image.pixels.color_at(x, y, 20),
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
        assert_eq!(
            image.pixels.color_at(0, 0, 20),
            Rgb {
                r: 1.0,
                g: 1.0,
                b: 1.0
            }
        );
    }
}
