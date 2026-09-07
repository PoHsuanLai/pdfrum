# pdfrum-common

The vocabulary every other crate in the workspace shares, and nothing else:
no parsing, no PDF objects, no I/O. Four ideas — the damage channel, the
ceilings, the version header, and a zero-based page index — plus
[`kurbo`](https://crates.io/crates/kurbo) re-exported so geometry is one type
across the workspace rather than one per crate.

```rust
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};

let mut diags = Diagnostics::default();
diags.record(Severity::Recovered, DiagKind::XrefRebuilt, Some(1234));
assert_eq!(diags.len(), 1);

assert_eq!(Limits::default().max_object_nesting, 64);
```

**[`Diagnostics`] is why `Err` is rare.** A PDF a browser would open is
routinely malformed, so damage the reader survived is *recorded*, not
returned: [`Severity::Recovered`] is a repair the reader is confident about
(a rebuilt xref, a `/Length` fixed by scanning for `endstream`), and
[`Severity::Suspicious`] is "we proceeded, but information was dropped".
An `Err` means the file could not be opened at all. [`DiagKind`] is
`#[non_exhaustive]` and a variant only exists where a crate actually records
it — a variant with no recording site reads as a promise the library does not
keep.

**[`Limits`] is the untrusted-input budget**, passed by reference down every
call chain rather than read from a global, so two documents in one process
can be opened under different ceilings. [`Deadline`] is the wall-clock half
of the same idea and is separately constructible ([`Deadline::manual`]) —
`Instant::now` panics on `wasm32`, where cancellation is the only stop.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
