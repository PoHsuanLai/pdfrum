# pdfrum-anyrender

[anyrender](https://crates.io/crates/anyrender) scenes written as vector PDF
pages through `pdfrum-edit`. Paths stay paths, gradients stay gradients, text
stays text in embedded, subsetted fonts, and images keep their source
encoding where PDF allows (a JPEG as-is, an opaque PNG's compressed data
as-is).

A page is painted into anyrender's recording `Scene` first, by whatever
renders through anyrender. [`write`] then embeds every face and image the
scenes use and replays each scene onto a pdfrum canvas: anyrender's push/pop
layers become nested saved graphics states, the only shape a pdfrum canvas
lets a content stream take.

```rust
use anyrender::Scene;
use pdfrum_anyrender::{GlyphArea, Page, Sources, write};
use peniko::kurbo::{Affine, Rect, Size};

let page = Page {
    size: Size::new(200.0, 100.0),
    scene: Scene::default(),
    placement: Affine::IDENTITY,
    clip: Rect::new(0.0, 0.0, 200.0, 100.0),
    area: GlyphArea::Anywhere,
};
let written = write(&[page], &Sources::default())?;
assert!(written.bytes.starts_with(b"%PDF-"));
# Ok::<(), pdfrum_edit::Error>(())
```

anyrender's glyph runs carry glyph ids, not the text they were shaped from.
Hand the text over in [`Sources::texts`], keyed by the run as the scene holds
it ([`RunKey`]), to make words selectable; a run without an entry falls back
to the font's own cmap, reversed ([`GlyphTally`] counts both).

## What is simplified

Box shadows are dropped; filters and backdrop filters are ignored;
compositing operators other than source-over paint as source-over; a sweep
gradient, and a gradient on a stroke or on text, paints as its stops' average
colour; gradients pad at their ends whatever extend mode they name, and
interpolate in sRGB; an image brush draws once, clipped to its shape.

## Test font

`tests/fixtures/noto-serif-latin.ttf` is a Latin subset of Noto Serif,
licensed under the SIL Open Font License 1.1
(`tests/fixtures/OFL-noto-serif.txt`). It is a test fixture and is not part
of the published crate's code.
