//! A PNG encoder, because an embedded image has to be one and the `png`
//! crate is tool-and-test-only (DEPS.md).
//!
//! SVG's `<image>` reads a `data:` URI, and the two raster formats every
//! renderer accepts there are PNG and JPEG. JPEG is lossy and has no alpha,
//! which rules it out for a composited region whose whole content is its
//! alpha channel; PNG is the only faithful choice. Adding a dependency for it
//! is not: `pdfrum-svg` ships with nothing beyond the geometry vocabulary the
//! engine already speaks, so the encoder is written here.
//!
//! # What it does and does not do
//!
//! It writes a **valid, lossless, uncompressed** PNG: the eight-byte
//! signature, `IHDR`, one `IDAT` holding a zlib stream of *stored* deflate
//! blocks (RFC 1951 §3.2.4 — the compression method every deflate decoder
//! must implement), and `IEND`. Every scanline uses filter type 0.
//!
//! Stored blocks mean the payload is roughly the raw pixels plus 0.1%, where
//! a real compressor would often manage a fraction of that, and base64 adds a
//! third again. That is the price of the closed dependency set, it is paid
//! only by the regions the walk delivers as pixels, and it costs nothing in
//! fidelity — every byte is the byte the engine produced. A caller who wants
//! the file smaller recompresses the `data:` payloads afterwards; the pixels
//! are already exact.

/// The largest a stored deflate block can be: its length is a `u16`.
const MAX_STORED_BLOCK: usize = 0xffff;

/// `bytes` as a PNG file, eight-bit RGBA at `width` x `height`.
///
/// `None` when `bytes` is not exactly `width * height * 4` long or either
/// dimension is zero — a caller with a malformed pixmap gets no element
/// rather than a corrupt one.
#[must_use]
pub fn encode_rgba(width: u32, height: u32, bytes: &[u8]) -> Option<Vec<u8>> {
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || bytes.len() != expected {
        return None;
    }

    // Each scanline is prefixed with its filter byte, which is 0 — "none".
    // Filtering exists to help the compressor, and stored blocks have no
    // compressor to help.
    let stride = (width as usize).checked_mul(4)?;
    let mut raw = Vec::with_capacity(expected.checked_add(height as usize)?);
    for row in bytes.chunks_exact(stride) {
        raw.push(0u8);
        raw.extend_from_slice(row);
    }

    let mut out = Vec::with_capacity(raw.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // 8 bits per channel, colour type 6 (RGBA), deflate, adaptive filtering,
    // no interlace — the only combination this encoder writes.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, *b"IHDR", &ihdr);
    chunk(&mut out, *b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, *b"IEND", &[]);
    Some(out)
}

/// One PNG chunk: length, type, payload, CRC over type and payload.
fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(&kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// `data` as a zlib stream (RFC 1950) of stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 0x8000 * 5 + 16);
    // CMF/FLG: deflate, 32 KiB window, no preset dictionary, and the check
    // bits chosen so the pair is a multiple of 31.
    out.extend_from_slice(&[0x78, 0x01]);
    // An empty payload still needs one (final, empty) block.
    let mut chunks = data.chunks(MAX_STORED_BLOCK).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    }
    while let Some(block) = chunks.next() {
        let last = u8::from(chunks.peek().is_none());
        let len = u16::try_from(block.len()).unwrap_or(u16::MAX);
        // BFINAL in bit 0, BTYPE 00 (stored) in bits 1-2, then LEN and its
        // one's complement, both little-endian.
        out.push(last);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32 (RFC 1950 §9), the zlib stream's trailing checksum.
fn adler32(data: &[u8]) -> u32 {
    // The largest prime below 65536, which is what keeps the sums from
    // overflowing before they are reduced.
    const BASE: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % BASE;
        b = (b + a) % BASE;
    }
    (b << 16) | a
}

/// The CRC-32 PNG chunks carry, computed a nibble at a time.
///
/// A nibble rather than a byte: the 16-entry table is small enough to build
/// on every call without measuring, which keeps the whole thing free of the
/// process-wide lazy table STYLE.md §1 forbids.
struct Crc32 {
    state: u32,
}

impl Crc32 {
    /// A fresh accumulator, pre-inverted as the algorithm requires.
    fn new() -> Self {
        Self { state: 0xffff_ffff }
    }

    /// Fold `data` in.
    fn update(&mut self, data: &[u8]) {
        // The reversed CRC-32 polynomial, 0xEDB88320, applied four bits at a
        // time from the low end.
        const TABLE: [u32; 16] = [
            0x0000_0000,
            0x1db7_1064,
            0x3b6e_20c8,
            0x26d9_30ac,
            0x76dc_4190,
            0x6b6b_51f4,
            0x4db2_6158,
            0x5005_713c,
            0xedb8_8320,
            0xf00f_9344,
            0xd6d6_a3e8,
            0xcb61_b38c,
            0x9b64_c2b0,
            0x86d3_d2d4,
            0xa00a_e278,
            0xbdbd_f21c,
        ];
        for byte in data {
            for shift in [0u32, 4] {
                let nibble = (u32::from(*byte) >> shift) & 0x0f;
                let index = ((self.state ^ nibble) & 0x0f) as usize;
                self.state = (self.state >> 4) ^ TABLE.get(index).copied().unwrap_or(0);
            }
        }
    }

    /// The finished value.
    fn finish(self) -> u32 {
        self.state ^ 0xffff_ffff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_reference_vector() {
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xcbf4_3926);
    }

    #[test]
    fn adler32_matches_the_reference_vector() {
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn a_mismatched_buffer_is_refused() {
        assert!(encode_rgba(2, 2, &[0; 15]).is_none());
        assert!(encode_rgba(0, 2, &[]).is_none());
    }

    #[test]
    fn the_file_has_the_signature_and_the_three_chunks() {
        let png = encode_rgba(1, 1, &[1, 2, 3, 4]).expect("a 1x1 pixmap encodes");
        assert_eq!(
            png.get(..8),
            Some(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a][..])
        );
        for kind in [&b"IHDR"[..], b"IDAT", b"IEND"] {
            assert!(
                png.windows(4).any(|w| w == kind),
                "{} is present",
                String::from_utf8_lossy(kind)
            );
        }
        // IEND is the last chunk, and its CRC is the file's last four bytes.
        assert_eq!(png.len(), 8 + 25 + (12 + 2 + 5 + 5 + 4) + 12);
    }

    /// The encoder's own output, decoded by `resvg`'s `tiny-skia`, is what
    /// the round trip depends on — so the pixels are checked against the
    /// bytes that went in.
    #[test]
    fn a_pixmap_round_trips_through_the_encoder() {
        let width = 3;
        let height = 2;
        let mut rgba = Vec::new();
        for i in 0..width * height {
            let v = u8::try_from(i * 40).unwrap_or(255);
            rgba.extend_from_slice(&[v, 255 - v, 128, 255]);
        }
        let png = encode_rgba(width, height, &rgba).expect("encodes");
        let decoded = tiny_skia::Pixmap::decode_png(&png).expect("a valid PNG");
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
        // `tiny-skia` premultiplies on decode; at full alpha that is identity.
        assert_eq!(decoded.data(), &rgba[..]);
    }

    #[test]
    fn a_payload_larger_than_one_stored_block_still_decodes() {
        // 100x200 RGBA is 80 000 bytes of pixels plus 200 filter bytes, which
        // is more than one 65535-byte stored block can hold.
        let (w, h) = (100u32, 200u32);
        // Opaque, so `tiny-skia`'s premultiplication on decode is the
        // identity and the bytes come back exactly as they went in.
        let rgba: Vec<u8> = [7u8, 9, 11, 255]
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect();
        let png = encode_rgba(w, h, &rgba).expect("encodes");
        let decoded = tiny_skia::Pixmap::decode_png(&png).expect("a valid PNG");
        assert_eq!((decoded.width(), decoded.height()), (w, h));
        assert_eq!(decoded.data(), &rgba[..]);
    }
}
