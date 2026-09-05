//! Shared plumbing for the fuzz targets.
//!
//! Only what more than one target needs: a length-prefixed splitter for
//! entry points that take several byte strings, and the `Limits` the whole
//! ring runs under.
//!
//! The `arbitrary` crate would do the splitting, but the fuzz ring's
//! dependency list stays as short as the library ring's — this is thirty
//! lines and it keeps the corpus format something a human can read in a hex
//! dump, which matters when triaging a crash file by hand.

/// A cursor over the fuzz input that hands out length-prefixed chunks.
///
/// Each `take` reads one length byte and returns that many bytes (clamped to
/// what is left). `rest` returns everything not yet consumed — the bulk
/// payload every target ends with.
#[derive(Debug)]
pub struct Split<'a> {
    bytes: &'a [u8],
}

impl<'a> Split<'a> {
    /// A cursor at the start of `bytes`.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// One byte of control data, or zero once the input is exhausted.
    pub fn byte(&mut self) -> u8 {
        match self.bytes.split_first() {
            Some((&b, rest)) => {
                self.bytes = rest;
                b
            }
            None => 0,
        }
    }

    /// A length-prefixed chunk: one length byte, then that many bytes.
    pub fn take(&mut self) -> &'a [u8] {
        let want = usize::from(self.byte());
        let n = want.min(self.bytes.len());
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        head
    }

    /// Everything not yet consumed.
    #[must_use]
    pub fn rest(self) -> &'a [u8] {
        self.bytes
    }
}

/// The caps every target runs under.
///
/// Deliberately much smaller than the library defaults for the output cap:
/// a fuzzer that is allowed to allocate a gigabyte finds OOMs, not bugs, and
/// libFuzzer's `-rss_limit_mb` would kill the process on an input that is
/// behaving exactly as designed. 1 MiB is the ceiling the filters brief
/// asks fuzz targets to use. Every other
/// field keeps its production value, because those *are* the limits under
/// test.
#[must_use]
pub fn limits() -> pdfrum_common::Limits {
    pdfrum_common::Limits {
        max_decoded_stream_len: 1 << 20,
        ..pdfrum_common::Limits::default()
    }
}

/// A diagnostics sink sized for a fuzz run.
///
/// Small: a target that records a million diagnostics is measuring `Vec`
/// growth, not the crate under test, and the sink's own overflow behaviour is
/// covered by `pdfrum-common`'s unit tests.
#[must_use]
pub fn diags() -> pdfrum_common::Diagnostics {
    pdfrum_common::Diagnostics::with_limit(64)
}
