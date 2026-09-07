# pdfrum-form

Interactive forms as a live session (ISO 32000-1 §12.7): events in,
appearance updates out. A click, a keystroke or a focus change goes in;
what changed on screen comes back. Nothing is rasterized here, and the field
appearance streams themselves are generated in `pdfrum-doc`.

```rust
use pdfrum_form::{FormSession, NoScripts};

let session = FormSession::new();
assert!(session.fields.is_empty());
let _cascade = NoScripts;
```

[`FormSession`] holds live field state across events; [`route::apply`] routes
one [`Event`] into it and returns a [`Response`] saying what changed, and the
caller paints. That split is why this crate has no window and no widget
chrome — an embedder owns the pixels, and the same session drives a GUI, a
headless fill and a test.

**A focused field draws from live editor state; an unfocused one falls back
to a generated appearance.** Those are two different sources for the same
rectangle, and the switch is what makes a caret and a partial selection
possible at all — a generated appearance stream has no notion of either.
Coordinates are page space, y-up from the crop-box origin, not device pixels.

**Two identifiers, and they are not interchangeable.** [`FieldId`] is
page-local, which is what a hit test on one page produces. `FieldRef::index`
is the document-wide name a script uses, because a `/AA` script naming a
field means the field anywhere in the document. A form whose fields span
pages needs both.

## Features

| feature | default | adds |
|---|:---:|---|
| `javascript` | off | the [boa](https://crates.io/crates/boa_engine)-backed [`Cascade`] — the document's own `/AA` scripts, actually run |

With `javascript` off, `boa_engine` is not in the dependency tree and a
document's scripts are data: [`NoScripts`] is the [`Cascade`] that runs
nothing, and every other behaviour of the session is unchanged.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
