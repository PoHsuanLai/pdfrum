# pdfrum-page

Content streams → `Op` list → page-object graph. Colorspaces, functions,
shadings 1–7, transparency.

```text
bytes ──parse_content──▶ Vec<Op> ──build_page──▶ Page
```

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_page::{Op, parse_content};

let ops = parse_content(b"0 0 100 50 re f", &Limits::default(), &mut Diagnostics::default());
assert_eq!(ops, vec![Op::Rectangle(0.0, 0.0, 100.0, 50.0), Op::Fill()]);
```

`parse_content` is infallible. Unknown operators are diagnostics.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
