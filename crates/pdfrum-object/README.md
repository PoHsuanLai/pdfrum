# pdfrum-object

PDF objects: `Object`, `Dict`, `Array`, `ObjRef`, `Name`, `Resolve`.

```rust
use pdfrum_object::{Array, Dict, NoResolve, Object, names};

let page = Dict::from_pairs([
    (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
    (names::RECT.clone(), Object::Array(Array::of([0, 0, 612, 792].map(Object::from)))),
]);
assert_eq!(page.rect(names::RECT, &NoResolve).width(), 612.0);
```

Accessors come in resolving and non-resolving pairs. Integers have two
readings (`4294967295` as int vs number).

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
