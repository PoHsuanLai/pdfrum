# pdfrum-cmap

**Predefined CJK CMap tables and embedded CMap parsing.**

CMaps answer two questions that PDF keeps in one object (ISO 32000-1 §9.7.5). A
`CMap` is a **byte decoder** — the rule that splits a text-showing string into
character codes, which may be one, two or four bytes wide and may mix widths
within a single string — and it is a **charcode→CID map**, turning each code
into an index into a character collection a CID font can draw.

Both ways a font can name one are here:

- **predefined** — `/Encoding /GB-EUC-H` selects one of 32 built-in CJK CMaps
  (plus `Identity-H` and `Identity-V`), whose compact static tables ship inside
  this crate. `from_encoding_name`.
- **embedded** — `/Encoding` is a stream holding a CMap program, read by
  `parse_embedded`.

```rust
use pdfrum_cmap::{CharCode, Cid, CodingScheme, from_encoding_name};
use pdfrum_common::Diagnostics;
use pdfrum_object::Name;

let mut diags = Diagnostics::default();
let cmap = from_encoding_name(&Name::from("Identity-H"), &mut diags);

// Identity-H reads fixed two-byte codes and maps each to itself.
assert_eq!(cmap.coding_scheme(), CodingScheme::TwoBytes);
let decoded: Vec<(CharCode, Cid)> = cmap.decode(&[0x00, 0x41, 0x30, 0x42]).collect();
assert_eq!(decoded, vec![
    (CharCode(0x0041), Cid(0x0041)),
    (CharCode(0x3042), Cid(0x3042)),
]);
```

Nothing here refuses to work. An unrecognised `/Encoding` name yields a CMap
that decodes two-byte codes and maps each to itself; a truncated multi-byte
code yields character code 0; a CMap program full of garbage yields whatever it
managed to say. Every recovery is recorded on a diagnostics sink rather than
raised, so a caller can see what was bent without having to handle it.

## Part of pdfrum

`pdfrum-cmap` is the CMap layer of [pdfrum](https://crates.io/crates/pdfrum), a
pure-Rust PDF engine. For a batteries-included API — text extraction and
rendering with CMaps resolved for you — use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
