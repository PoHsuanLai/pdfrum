# pdfrum-type1

Type 1 font programs (ISO 32000-1 §9.6.2): PFA and PFB containers, the
`eexec` and charstring encryptions, the Type 1 charstring interpreter, and
Multiple Master interpolation. This crate exists because Fontations does not
read the format — CFF and TrueType stay in
[`skrifa`](https://crates.io/crates/skrifa), and only the one format nothing
else handles lands here.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_type1::Type1Font;

# fn demo(pfb: &[u8]) -> Option<()> {
let font = Type1Font::parse(pfb, &Limits::default(), &mut Diagnostics::default()).ok()?;
let gid = font.name_to_gid("A")?;
let (outline, advance) = font.outline(gid)?;
let _ = (font.units_per_em(), font.font_matrix(), outline, advance);
# Some(())
# }
```

A Type 1 font is name-addressed, not index-addressed: its charstrings are a
dictionary keyed by glyph name, and the `/Encoding` array maps a byte to a
name rather than to a glyph number. [`Gid`] here is this crate's own stable
numbering over that dictionary, so [`Type1Font::code_to_gid`],
[`Type1Font::name_to_gid`] and [`Type1Font::unicode_to_gid`] are three
routes into one space — and [`Type1Font::glyph_name`] goes back, which is
what makes a `/Differences` encoding resolvable at all.

Outlines are in font units, `1000` per em by convention but read from the
`/FontMatrix` rather than assumed ([`Type1Font::units_per_em`],
[`Type1Font::font_matrix`]). Multiple Master fonts are not interpolated on
parse: [`Type1Font::mm_axes`] reports the design axes and
[`Type1Font::instantiate`] borrows an instance at a coordinate, so one parsed
font serves every weight a document asks for.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
