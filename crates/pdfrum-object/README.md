# pdfrum-object

The eight object types in ISO 32000-1 §7.3 — null, boolean, integer, real,
string, name, array, dictionary — plus streams and indirect references
(`N G R`). Construction and typed access only; no parsing lives here.

Resolution is one hop: a reference to a reference is absent. Integers have
two readings (`4294967295` as int vs number).

```rust
use pdfrum_object::{Array, Dict, NoResolve, Object, names};

let page = Dict::from_pairs([
    (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
    (names::RECT.clone(), Object::Array(Array::of([0, 0, 612, 792].map(Object::from)))),
]);
assert_eq!(page.rect(names::RECT, &NoResolve).width(), 612.0);
```

Accessors come in resolving and non-resolving pairs.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
