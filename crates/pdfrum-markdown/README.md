# pdfrum-markdown

Markdown, and layout-preserving plain text, from an interpreted page. Two
readings of the same page, chosen by what the file offers: a tagged PDF
(ISO 32000-1 §14.7) names its headings, paragraphs, lists, tables and
figures in the structure tree, and an untagged one is read by typography
instead — body size against heading scale, list markers, monospace runs as
code fences, running headers dropped.

```rust
use pdfrum_markdown::{Block, render};

let md = render(&[
    Block::Heading { level: 1, text: "Results".into() },
    Block::Paragraph("Ninety-nine percent agreement.".into()),
]);
assert_eq!(md, "# Results\n\nNinety-nine percent agreement.\n");
```

The pipeline is three named stages, and each is callable on its own:
[`page_lines`] extracts and groups drawn glyphs into [`Line`]s,
[`page_blocks`] turns those into [`Block`]s (through the structure tree when
there is one, [`heuristics`] when there is not), and [`render()`] writes
CommonMark. [`page_markdown`] is the three composed; [`page_layout`] stops
after the first and keeps the page's own geometry as spacing instead.

[`document_blocks`] is the whole-document entry point and is not a loop over
[`page_blocks`]: repeated header and footer bands are only detectable across
pages, so a line that recurs in the same band on page after page is dropped
there and could not be dropped one page at a time.

Images are indices, never bytes. `Block::Image` carries the image's position
in the page's drawing order — the order the facade's `Page::images` lists
them in — so [`render_with_images`] can link each one wherever the caller
actually wrote the file.

From the facade: `Page::markdown`, feature `markdown`.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
