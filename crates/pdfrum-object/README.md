# pdfrum-object

**PDF object model: Object enum, names, indirect-reference resolution.**

The eight PDF object types of ISO 32000 §7.3 as plain Rust values — the
`Object` enum (Null, Bool, Int, Real, String, Name, Array, Dict, Stream, Ref),
the `Dict` and `Array` containers with typed accessors, `ObjRef` for indirect
references, interned `Name` values with the dictionary-key constants a PDF
reader needs, and the `Resolve` trait that a document store implements to
answer indirect lookups.

Construction and typed access only. No parsing lives here, so the crate is
usable as the vocabulary for anything that reads or writes PDF structure
without buying into a particular file reader.

```rust
use pdfrum_object::{Array, Dict, NoResolve, Object, names};

let page = Dict::from_pairs([
    (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
    (
        names::RECT.clone(),
        Object::Array(Array::of([0, 0, 612, 792].map(Object::from))),
    ),
]);

assert_eq!(page.name(names::TYPE), Some(names::PAGE));
assert_eq!(page.rect(names::RECT, &NoResolve).width(), 612.0);
```

Two design points that real files force. **Resolution is one level, and which
accessors perform it is a choice per call site** — every accessor comes in a
resolving and a non-resolving flavour, because an indirect `/Prev` must be
ignored by a cross-reference reader while an indirect `/Length` must be chased.
**Integers have two readings**: a file that writes `4294967295` for a
permissions word means `-1` read as an integer and `4294967296.0` read as a
number, and the accessors reproduce both.

## Part of pdfrum

`pdfrum-object` is the object layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — documents, pages, rendering, text — use the `pdfrum`
facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
