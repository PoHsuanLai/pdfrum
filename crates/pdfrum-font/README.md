# pdfrum-font

**PDF font dictionaries, encodings, glyph mapping and outlines.**

Everything a PDF renderer or text extractor needs from ISO 32000-1 §9: font
dictionaries of every kind (Type1, TrueType, Type0, Type3, CID), encodings and
`/Differences`, `/ToUnicode`, the code→CID→GID mapping ladders, glyph outlines
and metrics through `skrifa`, substitution and fallback selection through
`fontdb`, and a per-session glyph cache.

A PDF font dictionary says almost nothing directly. It names a base font,
points at an encoding, and — usually — carries a program. Everything that
matters is derived: which glyph a byte selects, what character it stands for,
and how wide it is. Those three questions have *different* answers arrived at
by *different* ladders, and reproducing the ladders is what this crate is for.

`Font::decode` is the single entry point that answers all three at once,
yielding one `CharItem` per character code in a string. Everything above this
crate — rendering and text extraction alike — reads that stream and nothing
else.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_font::{FontCache, load};

fn demo(dict: &pdfrum_object::Dict, doc: &impl pdfrum_object::Resolve) -> Option<()> {
    let cache = FontCache::default();
    let mut diags = Diagnostics::default();
    let font = load(dict, doc, &cache, &Limits::default(), &mut diags)?;

    for item in font.decode(b"Hello") {
        let text: String = item.unicode.iter().collect();
        println!("code {:#x} -> glyph {} -> {text:?} ({}/1000 em)",
                 item.code.0, item.gid.0, item.width);
    }
    Some(())
}
```

A simple font *always* constructs, even with no program, no encoding and no
glyphs. Only a Type0 font can fail to load, in the four ways `Error` names.
Everything else is a diagnostic and a best-effort result.

## Part of pdfrum

`pdfrum-font` is the font layer of [pdfrum](https://crates.io/crates/pdfrum), a
pure-Rust PDF engine. For a batteries-included API — pages, rendering, text —
use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
