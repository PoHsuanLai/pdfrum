//! Which part of the page a render covers: all of it, or one device-space
//! tile.

use crate::device::MAX_TARGET_DIMENSION;
use crate::error::Error;

/// A rectangle of whole device pixels, measured from the top-left corner of
/// the page's full device box (the box [`target_size`](crate::walk::target_size)
/// sizes under the render transform).
///
/// Validated at construction: neither side is zero, neither exceeds
/// [`MAX_TARGET_DIMENSION`] — the largest pixmap a backend allocates — and
/// the far edges fit in a `u32`. Whether it lies inside a particular page's
/// device box is the render's to check, since only the render knows the box.
///
/// ```
/// use pdfrum_render::DeviceRect;
///
/// let tile = DeviceRect::new(512, 256, 256, 256)?;
/// assert_eq!((tile.x(), tile.y(), tile.width(), tile.height()), (512, 256, 256, 256));
/// assert!(DeviceRect::new(0, 0, 0, 10).is_err());
/// # Ok::<(), pdfrum_render::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl DeviceRect {
    /// The rectangle with its top-left corner at (`x`, `y`) and the given
    /// size, in device pixels.
    ///
    /// # Errors
    ///
    /// [`Error::TargetEmpty`] when `width` or `height` is zero, and
    /// [`Error::TargetTooLarge`] when either exceeds [`MAX_TARGET_DIMENSION`]
    /// or when `x + width` or `y + height` overflows a `u32`.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Result<DeviceRect, Error> {
        if width == 0 || height == 0 {
            return Err(Error::TargetEmpty { width, height });
        }
        if width > MAX_TARGET_DIMENSION
            || height > MAX_TARGET_DIMENSION
            || x.checked_add(width).is_none()
            || y.checked_add(height).is_none()
        {
            return Err(Error::TargetTooLarge {
                width,
                height,
                limit: MAX_TARGET_DIMENSION,
            });
        }
        Ok(DeviceRect {
            x,
            y,
            width,
            height,
        })
    }

    /// The left edge, in pixels from the left of the page's device box.
    #[must_use]
    pub fn x(&self) -> u32 {
        self.x
    }

    /// The top edge, in pixels from the top of the page's device box.
    #[must_use]
    pub fn y(&self) -> u32 {
        self.y
    }

    /// The width in pixels; also the width of the pixmap a render of this
    /// rectangle produces.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels; also the height of the pixmap a render of this
    /// rectangle produces.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// How much of the page a render draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Region {
    /// The page's whole device box, which must fit [`MAX_TARGET_DIMENSION`]
    /// on both axes.
    #[default]
    Whole,
    /// Only this rectangle of the device box, into a pixmap of exactly its
    /// size. The page's full device size is not allocated and so is not
    /// bound by [`MAX_TARGET_DIMENSION`]; the rectangle must lie inside it,
    /// else [`Error::RegionOutOfBounds`].
    ///
    /// # Seams
    ///
    /// The walk is the whole page's: the page matrix, the pixel rounding of
    /// every object's extent, the glyph snap and the culling are computed in
    /// the whole render's frame, and only the target is the rectangle's size.
    /// So a tile's pixels are the whole render's, except where the
    /// rasterizer's own arithmetic depends on where the target starts:
    ///
    /// - `vello_cpu` flattens in `f32`, and a tile's geometry is the whole
    ///   page's shifted by whole pixels, which can round differently. Tiles
    ///   are byte-identical to the whole render in this crate's and the
    ///   facade's tests, and otherwise differ by one count on an occasional
    ///   antialiased edge pixel — never by a pixel of content.
    /// - `tiny-skia` and the AGG port integrate coverage in a way that
    ///   depends on where an edge enters the target, so a seam can differ by
    ///   a few counts along an edge.
    /// - A transparency group, soft mask or pattern cell that the rectangle's
    ///   edge cuts is drawn in a buffer that starts at the edge, not at the
    ///   group's own corner; the same integer shift, the same few counts.
    ///
    /// Tiles on a grid are rendered the same way every time: the output is a
    /// function of the page, the options and the rectangle.
    Rect(DeviceRect),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_is_non_empty_and_within_the_backend_limit() {
        assert!(DeviceRect::new(0, 0, 1, 1).is_ok());
        assert!(DeviceRect::new(u32::MAX - 65535, 0, 65535, 1).is_ok());
        assert_eq!(
            DeviceRect::new(0, 0, 0, 5),
            Err(Error::TargetEmpty {
                width: 0,
                height: 5
            })
        );
        assert!(matches!(
            DeviceRect::new(0, 0, 65536, 5),
            Err(Error::TargetTooLarge { .. })
        ));
        assert!(matches!(
            DeviceRect::new(u32::MAX, 0, 1, 1),
            Err(Error::TargetTooLarge { .. })
        ));
    }
}
