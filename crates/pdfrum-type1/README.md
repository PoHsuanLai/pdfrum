# pdfrum-type1

PFA/PFB, `eexec`, Type 1 charstrings, Multiple Master. Fontations does not
read this format; CFF/TTF stay in `skrifa`.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_type1::Type1Font;

# fn demo(pfb: &[u8]) -> Option<()> {
let font = Type1Font::parse(pfb, &Limits::default(), &mut Diagnostics::default()).ok()?;
let _ = font;
# Some(())
# }
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
