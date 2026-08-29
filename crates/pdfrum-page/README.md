# pdfrum-page

**Content-stream interpreter, graphics state, colorspaces, shadings.**

Page content semantics, per ISO 32000 §8: content-stream operators parsed to a
typed `Op` list, the interpreter folding those ops into a typed page-object
graph, the graphics state, colorspaces (device, ICC, Indexed, Separation, Lab),
PDF functions (types 0, 2, 3 and 4), patterns and shadings (types 1 through 7),
and the transparency model — groups, soft masks, blend modes.

Content interpretation is a pipeline of two pure functions, which is the whole
design:

```text
bytes ──parse_content──▶ Vec<Op> ──build_page──▶ Page { objects, boxes }
```

`parse_content` is infallible and needs nothing but bytes: it tokenizes, fills
a sixteen-slot operand ring, and emits one `Op` per recognised operator.
`build_page` is the fold that turns those operators into page objects, and is
where resources, the graphics-state stack and form recursion live. Separating
them means a content stream can be inspected, diffed and fuzzed without a
document, and a page can be built from synthesized operators without bytes.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_page::{Op, parse_content};

let mut diags = Diagnostics::default();
let ops = parse_content(b"0 0 100 50 re f", &Limits::default(), &mut diags);
assert_eq!(ops, vec![Op::Rectangle(0.0, 0.0, 100.0, 50.0), Op::Fill()]);
```

**Damage is data, not failure.** An unknown operator, a colorspace that will
not load, a function whose `/Domain` is missing, an image whose bit depth is
nonsense — each records a diagnostic and yields a best-effort result, because
that behaviour is what makes broken PDFs render at all.

## Part of pdfrum

`pdfrum-page` is the content layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — open a file and render or extract from it directly —
use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
