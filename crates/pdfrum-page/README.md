# pdfrum-page

The content layer (ISO 32000-1 §8). A page's `/Contents` is a stream of
painting operators in user space — points, origin bottom-left, y-up — and
this crate turns those bytes into a page-object graph that both the renderer
and the text extractor walk. Colour spaces, PDF functions, shadings 1 through
7, transparency groups, soft masks, patterns and Type 3 glyph procedures all
resolve here.

```text
bytes ──parse_content──▶ Vec<Op> ──build_page──▶ Page
```

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_page::{Op, parse_content};

let ops = parse_content(b"0 0 100 50 re f", &Limits::default(), &mut Diagnostics::default());
assert_eq!(ops, vec![Op::Rectangle(0.0, 0.0, 100.0, 50.0), Op::Fill()]);
```

**The two stages are separate pure functions, and that is the crate's main
design decision.** [`parse_content`] tokenizes and emits one [`Op`] per
recognised operator, knowing nothing about resources or state.
[`build_page`] is the fold that turns those operators into page objects, and
it is where `/Resources`, the graphics-state stack and form-XObject recursion
live. Splitting them means a content stream can be inspected, diffed and
fuzzed with no document behind it, and a page can be built from synthesized
operators with no bytes.

[`parse_content`] is infallible. An unrecognised operator, a wrong operand
count, or a truncated stream is a diagnostic and a skipped `Op` — a content
stream is the part of a PDF most likely to be damaged, and refusing to parse
it would mean refusing to draw pages that every viewer draws.

## Features

| feature | adds |
|---|---|
| `jpx` | JPEG 2000 images ([`hayro-jpeg2000`](https://crates.io/crates/hayro-jpeg2000)) |
| `jbig2` | JBIG2 images ([`hayro-jbig2`](https://crates.io/crates/hayro-jbig2)) |
| `ccitt` | CCITT Group 3/4 fax images |

Flate and JPEG are always present. A stream in a codec this build left out
decodes to a recorded failure, and the rest of the page still draws — the
facade turns all three on together as `codecs-all`.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
