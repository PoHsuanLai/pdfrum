//! Shared plumbing: a length-prefixed splitter, and the `Limits` the ring
//! runs under.

/// Length-prefixed chunks: one length byte, then that many bytes. `rest` is
/// everything not yet consumed.
#[derive(Debug)]
pub struct Split<'a> {
    bytes: &'a [u8],
}

impl<'a> Split<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// One control byte, or zero once the input is exhausted.
    pub fn byte(&mut self) -> u8 {
        match self.bytes.split_first() {
            Some((&b, rest)) => {
                self.bytes = rest;
                b
            }
            None => 0,
        }
    }

    /// One length byte, then that many bytes (clamped to what is left).
    pub fn take(&mut self) -> &'a [u8] {
        let want = usize::from(self.byte());
        let n = want.min(self.bytes.len());
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        head
    }

    #[must_use]
    pub fn rest(self) -> &'a [u8] {
        self.bytes
    }
}

/// Caps. `max_decoded_stream_len` is 1 MiB so the fuzzer finds bugs, not
/// OOMs; production is 1 GiB. Everything else keeps its production value.
#[must_use]
pub fn limits() -> pdfrum_common::Limits {
    pdfrum_common::Limits {
        max_decoded_stream_len: 1 << 20,
        ..pdfrum_common::Limits::default()
    }
}

/// Diagnostics sink. Overflow is covered by `pdfrum-common`'s unit tests.
#[must_use]
pub fn diags() -> pdfrum_common::Diagnostics {
    pdfrum_common::Diagnostics::with_limit(64)
}
