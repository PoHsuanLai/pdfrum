//! A packed one-bit image: what JBIG2 and a stencil mask decode to.

/// A one-bit-per-pixel image, packed MSB-first with each row starting on a
/// byte boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row, `width.div_ceil(8)`.
    pub row_bytes: usize,
    /// The packed bits. A set bit is **black**, matching the JBIG2 convention
    /// and PDF's default `/Decode` for a one-bit image.
    pub bits: Vec<u8>,
}

impl BitImage {
    /// Whether the pixel at `(x, y)` is black.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> bool {
        let Ok(row) = usize::try_from(y) else {
            return false;
        };
        let Ok(col) = usize::try_from(x) else {
            return false;
        };
        let Some(byte) = self.bits.get(row * self.row_bytes + col / 8) else {
            return false;
        };
        (byte >> (7 - (col % 8))) & 1 == 1
    }
}
