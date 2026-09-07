//! Stream objects (ISO 32000-1 §7.3.8) and the zero-copy window their data
//! lives in.

use std::fmt;
use std::ops::{Deref, Range};
use std::sync::Arc;

use bytes::Bytes;

use crate::{Dict, Error};

/// A window into a shared byte buffer.
///
/// The document's bytes are held once and every stream is a window into them,
/// so opening a file costs one copy no matter how many streams it holds. Three
/// buffers occur in practice: the file itself, a decrypted replacement for one
/// stream's bytes, and a decoded object stream's payload.
///
/// Backed by [`bytes::Bytes`], whose owner pointer is separate from its data
/// pointer. That is what lets [`ByteSpan::from`] adopt a `Vec<u8>`'s allocation
/// instead of copying it — `Arc<[u8]>` cannot, because its refcounts live
/// inline with the payload. A window is a refcount bump, never an allocation,
/// so [`subspan`](Self::subspan) is the cheap way to carve a file up.
///
/// Decoded (filtered) data is deliberately *not* cached here — the page layer
/// owns those caches, keyed by the reference that produced them.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_object::ByteSpan;
///
/// let file: Arc<[u8]> = Arc::from(&b"%PDF-1.7 stream-bytes"[..]);
/// let span = ByteSpan::new(file, 9..21).unwrap();
/// assert_eq!(&*span, b"stream-bytes");
/// assert_eq!(span.len(), 12);
/// ```
#[derive(Clone)]
pub struct ByteSpan {
    buf: Bytes,
    /// Where `buf` begins in the buffer it was carved from. Carried because
    /// `Bytes` does not record it and [`range`](Self::range) publishes it.
    start: usize,
}

impl ByteSpan {
    /// A window covering `range` of `file`.
    ///
    /// # Errors
    ///
    /// [`Error::SpanOutOfBounds`] when the range runs past the buffer or ends
    /// before it starts. Ranges come from `/Length` values in untrusted
    /// files, so this is checked rather than trusted.
    pub fn new(file: Arc<[u8]>, range: Range<usize>) -> Result<Self, Error> {
        // Checked before slicing, never delegated to `Bytes::slice`: that
        // panics where this must return, and the ranges are untrusted.
        if range.start > range.end || range.end > file.len() {
            return Err(Error::SpanOutOfBounds {
                start: range.start,
                end: range.end,
                len: file.len(),
            });
        }
        let start = range.start;
        Ok(Self {
            buf: Bytes::from_owner(file).slice(range),
            start,
        })
    }

    /// A window over a whole buffer.
    #[must_use]
    pub fn whole(file: Arc<[u8]>) -> Self {
        Self {
            buf: Bytes::from_owner(file),
            start: 0,
        }
    }

    /// An empty window.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            buf: Bytes::new(),
            start: 0,
        }
    }

    /// The bytes in the window.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Number of bytes in the window.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the window is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Where the window sits in its backing buffer.
    #[must_use]
    pub fn range(&self) -> Range<usize> {
        self.start..self.start.saturating_add(self.buf.len())
    }

    /// A sub-window, with offsets relative to this window's start.
    ///
    /// # Errors
    ///
    /// [`Error::SpanOutOfBounds`] when `range` leaves this window.
    pub fn subspan(&self, range: Range<usize>) -> Result<Self, Error> {
        if range.start > range.end || range.end > self.len() {
            return Err(Error::SpanOutOfBounds {
                start: range.start,
                end: range.end,
                len: self.len(),
            });
        }
        let start = self.start.saturating_add(range.start);
        Ok(Self {
            buf: self.buf.slice(range),
            start,
        })
    }
}

impl Deref for ByteSpan {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl AsRef<[u8]> for ByteSpan {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl PartialEq for ByteSpan {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for ByteSpan {}

impl fmt::Debug for ByteSpan {
    /// Prints the window's shape rather than its bytes: stream payloads run
    /// to megabytes and a `Debug` dump of an object tree must stay readable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ByteSpan")
            .field("len", &self.len())
            .field("at", &self.start)
            .finish_non_exhaustive()
    }
}

impl From<Arc<[u8]>> for ByteSpan {
    /// Shares the buffer; nothing is copied.
    fn from(file: Arc<[u8]>) -> Self {
        Self::whole(file)
    }
}

impl From<Vec<u8>> for ByteSpan {
    /// Adopts the vector's allocation; nothing is copied.
    fn from(bytes: Vec<u8>) -> Self {
        Self {
            buf: Bytes::from(bytes),
            start: 0,
        }
    }
}

/// A stream object: a dictionary describing bytes, plus the bytes.
///
/// The data is the *raw* payload as it sits in the file — filters have not
/// been applied and encryption has, if the document was encrypted. Its length
/// is authoritative: a `/Length` in the dictionary that disagreed with the
/// bytes found before `endstream` was already repaired by the reader, which
/// leaves the dictionary untouched and records a diagnostic.
///
/// ```
/// use pdfrum_object::{ByteSpan, Dict, Object, Stream, names};
///
/// let stream = Stream {
///     dict: Dict::from_pairs([(names::LENGTH.clone(), Object::Int(3))]),
///     data: ByteSpan::from(b"abc".to_vec()),
/// };
/// assert_eq!(&*stream.data, b"abc");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    /// The stream's dictionary: `/Length`, `/Filter`, `/DecodeParms`, and
    /// whatever the stream's own type adds.
    pub dict: Dict,
    /// The raw stream data.
    pub data: ByteSpan,
}

impl Stream {
    /// A stream from a dictionary and its raw bytes.
    #[must_use]
    pub fn new(dict: Dict, data: ByteSpan) -> Self {
        Self { dict, data }
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::sync::Arc;

    use super::ByteSpan;
    use crate::Error;

    #[test]
    fn window_covers_only_its_range() {
        let file: Arc<[u8]> = Arc::from(&b"0123456789"[..]);
        let span = ByteSpan::new(Arc::clone(&file), 2..5).unwrap();
        assert_eq!(&*span, b"234");
        assert_eq!(span.len(), 3);
        assert!(!span.is_empty());
        assert_eq!(span.range(), 2..5);
    }

    #[test]
    fn out_of_bounds_ranges_are_refused_not_clamped() {
        let file: Arc<[u8]> = Arc::from(&b"0123"[..]);
        assert_eq!(
            ByteSpan::new(Arc::clone(&file), 0..5),
            Err(Error::SpanOutOfBounds {
                start: 0,
                end: 5,
                len: 4
            })
        );
        let reversed = Range { start: 3, end: 1 };
        assert!(ByteSpan::new(Arc::clone(&file), reversed).is_err());
        assert!(ByteSpan::new(file, 4..4).is_ok());
    }

    #[test]
    fn subspans_are_relative_and_bounded() {
        let span = ByteSpan::from(b"0123456789".to_vec());
        let inner = span.subspan(2..5).unwrap();
        assert_eq!(&*inner, b"234");
        assert_eq!(&*inner.subspan(1..2).unwrap(), b"3");
        assert!(inner.subspan(0..4).is_err());
    }

    #[test]
    fn spans_compare_by_content_not_by_backing_buffer() {
        let a = ByteSpan::from(b"abc".to_vec());
        let b = ByteSpan::new(Arc::from(&b"xxabcxx"[..]), 2..5).unwrap();
        assert_eq!(a, b);
        assert!(ByteSpan::empty().is_empty());
    }
}
