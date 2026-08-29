# pdfrum-type1

**Type 1 font parser: PFA/PFB and charstring interpretation.**

Type 1 font programs, end to end: PFA and PFB containers, `eexec`-encrypted
private dictionaries, Type 1 charstring interpretation to outlines, the
built-in encoding, and **Multiple Master** interpolation.

This is the one classic outline format the Fontations stack does not read
whole — bare CFF, CFF2, OpenType/CFF, TrueType and Type 2 charstrings are all
`skrifa`'s business, and deliberately not this crate's.

`Type1Font::parse` sniffs the container, decrypts, and reads the program into a
record. Nothing is lazy and nothing is cached, so a `Type1Font` is a plain
value — `Send`, `Sync`, and cheap to hold. Multiple Master arrives through
`Type1Font::instantiate`, which takes *design* coordinates (a weight of
50–1450, a width of 100–900) and returns an instance whose outlines are blended
at that point.

```rust
use pdfrum_common::Diagnostics;
use pdfrum_type1::Type1Font;

fn demo(pfb: &[u8]) -> Option<()> {
    let mut diags = Diagnostics::default();
    let font = Type1Font::parse(pfb, &Default::default(), &mut diags).ok()?;

    // Character code to glyph, through the font's built-in encoding.
    let gid = font.code_to_gid(b'A')?;
    let (outline, _advance) = font.outline(gid)?;
    assert!(!outline.is_empty());

    // A Multiple Master face draws at whatever weight is asked for.
    if let Some(axes) = font.mm_axes() {
        let bold = font.instantiate(&[axes[0].max])?;
        assert!(bold.outline(gid).is_some());
    }
    Some(())
}
```

A Type 1 font effectively never fails to construct. `parse` returns `Err` only
when the blob has no private section or no `/CharStrings` at all; a truncated
PFB segment, hex that stops mid-byte, a charstring that runs off its end, or a
Multiple Master declaration whose parts disagree each yield a best-effort font
plus a diagnostic. A glyph whose charstring cannot be interpreted to completion
still returns the partial outline.

## Part of pdfrum

`pdfrum-type1` is the Type 1 layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine, where it
reads embedded `/FontFile` programs and instantiates the Multiple Master
fallback faces at the end of the substitution ladder. For a batteries-included
PDF API, use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
