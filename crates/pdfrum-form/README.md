# pdfrum-form

Interactive forms (ISO 32000-1 §12.7): events in, appearance updates out.
Nothing is rasterized here; field appearances live in `pdfrum-doc`.

A [`FormSession`] holds live field state across events. [`apply`] returns
what changed; the caller paints. Coordinates are page space, y-up from the
crop-box origin. A focused field draws from live editor state; an unfocused
one falls back to a generated appearance. `FieldId` is page-local;
`FieldRef::index` is the document-wide name a script uses.

JavaScript is the default-off `javascript` feature ([boa](https://boajs.dev/)).
Off, scripts are data.

```rust
use pdfrum_form::{FormSession, NoScripts};

let session = FormSession::new();
assert!(session.fields.is_empty());
let _cascade = NoScripts;
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
