# pdfrum-object

The eight object types of ISO 32000-1 §7.3 — null, boolean, integer, real,
string, name, array, dictionary — plus streams (a dictionary with bytes) and
indirect references (`N G R`). Construction and typed access only: nothing in
this crate reads a byte of a file, which is what lets a page, a font or an
annotation be built from synthesized objects in a test with no document
behind it.

```rust
use pdfrum_object::{Array, Dict, NoResolve, Object, names};

let page = Dict::from_pairs([
    (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
    (names::RECT.clone(), Object::Array(Array::of([0, 0, 612, 792].map(Object::from)))),
]);
assert_eq!(page.rect(names::RECT, &NoResolve).width(), 612.0);
```

**Resolution is one hop, deliberately.** A reference whose target is itself a
reference reads as absent rather than being chased, because a file can point
an object at itself and a resolver that loops is a hang on untrusted input.
Accessors come in resolving and non-resolving pairs so a caller says which it
wants at every site; [`NoResolve`] is the resolver for values that cannot
contain references, and it is a type rather than an `Option` so the choice is
visible in the signature.

Numbers have two readings and both are needed. `4294967295` is a valid
integer object and a valid real, and a `/Length` reading it as one while an
xref offset reads it as the other is a real file's real behaviour — so
integer and number access are separate methods rather than one lossy
conversion.

[`names`](mod@names) is the workspace's one declaration site for the specification's
dictionary keys. A key spelled there is never spelled again at a use site, so
a typo cannot silently produce a lookup that never matches — the failure mode
of string keys is a missing feature, not a compile error. `Name` is a
`Cow<'static, [u8]>`, so a constant costs no allocation, and `#xx` escapes
are resolved on construction so two spellings of one key compare equal.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
