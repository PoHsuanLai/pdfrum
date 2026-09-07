# pdfrum-cmap

A CMap does two jobs at once for a composite font (ISO 32000-1 §9.7.5): it
splits a show-operator's string into character codes one, two, three or four
bytes wide, and it maps each code to a CID. The split is the part a byte
oriented reader gets wrong — code widths vary *within* one string, so a CMap
has to be consulted to know where the next code even begins.

```rust
use pdfrum_cmap::from_encoding_name;
use pdfrum_common::Diagnostics;
use pdfrum_object::Name;

let cmap = from_encoding_name(&Name::from("Identity-H"), &mut Diagnostics::default());
assert!(!cmap.is_vertical());
let cids: Vec<_> = cmap.decode(&[0x00, 0x41, 0x00, 0x42]).collect();
assert_eq!(cids.len(), 2);
```

59 named tables over 32 decoder stems ship compiled in ([`predefined`]),
plus [`parse_embedded`] for a CMap the file carries as a stream.
[`from_encoding_name`] is the `/Encoding` entry point: an unknown name is a
diagnostic and a one-byte identity CMap, never an error, because a font whose
encoding cannot be resolved still has to draw something.

Inheritance runs through three live mechanisms and they are not
interchangeable. The built-in tables chain among themselves, which is what
makes `GB-EUC-V` a thin override of `GB-EUC-H`. `usecmap` inside an embedded
program is resolved by [`parse_embedded`]. The `/UseCMap` key of an
`/Encoding` stream's dictionary is attached by [`inherit_from`] and
supersedes the operator. The last two are §9.7.5.3's two channels, and both
are child-wins: a code the child maps is the child's answer, and only a code
it maps to nothing reaches the parent. The oracle implements neither.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
