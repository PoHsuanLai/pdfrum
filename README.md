<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
  <img src="docs/assets/banner-light.svg" alt="pdfrum" width="100%">
</picture>

**A composable PDF library built in Rust.**

pdfrum is a modern, thread-safe PDF library — modular stages you compose,
CPU or GPU backends, no `unsafe` — tested against PDFium. For your UI, RAG
pipeline, or any app that has to open a PDF file.

```toml
pdfrum = "0.1"
```

```rust
use pdfrum::{Document, RenderOptions, VelloCpuBackend};

let doc = Document::open("report.pdf")?;
for page in doc.pages() {
    let pixmap = page.render(&VelloCpuBackend::new(), &RenderOptions::scaled(2.0))?;
    let text = page.text().to_string();
    println!("page {}: {}×{}, {} chars",
        page.index(), pixmap.width(), pixmap.height(), text.len());
}
# Ok::<(), pdfrum::Error>(())
```

## What you can do with pdfrum

Open a file once. Ask it questions (Rendering is one of them).

- **Pages** — boxes, rotation, the object graph a page paints
- **Text** — reading order, search, selection, words with geometry
- **Images, fonts, attachments, signatures** — as stored, decoded when that
  is useful
- **Forms** — read values, fill them, run a live session
- **Outline, links, annotations, tagged structure**
- **Pixels** — name a rasterizer; defaults to `vello-cpu`
- **A new file** — edit, stamp, merge, flatten, save full or incremental

A file a browser would open, we open, and tell you where its broken.

## ![Pdfrum CLI](docs/assets/cli/pdfrum-cli.gif)

```sh
pdfrum preview gradients.pdf
pdfrum stamp text gradients.pdf DRAFT --angle 30 --opacity 0.4 --size 72 -o stamped.pdf
pdfrum preview stamped.pdf
pdfrum view stamped.pdf
pdfrum search ISO paper.pdf
```

The rest of the catalog — `info`, `pages`, `forms`, `render`,
`serve --stdio` / `--mcp` — is [`pdfrum-cli`](crates/pdfrum-cli/README.md).

```sh
cargo install pdfrum-cli
```

## Attributes

| | |
|---|---|
| Safe | `unsafe` is forbidden in the library. (The C ABI is the one exception, and only at `extern "C"`). |
| Pure Rust | no C/C++ in a library build |
| Thread-safe | every public type is `Send + Sync` |
| JavaScript | The `javascript` feature runs the document's own scripts via the [boa engine](https://crates.io/crates/boa_engine) |
| Not a viewer | no window, caret, or widget chrome. No XFA. |

Share a `Document` across threads; give each worker its own `RenderSession`.

```rust
use rayon::prelude::*;
use pdfrum::{Document, RenderOptions, RenderSession, VelloCpuBackend};

let doc = Document::open("big.pdf")?;
let pages: Vec<_> = doc.pages().collect();
let backend = VelloCpuBackend::new();
let pixmaps: Vec<_> = pages
    .par_iter()
    .map_init(RenderSession::new, |session, page| {
        page.render_on(&backend, &RenderOptions::default(), session)
    })
    .collect::<Result<_, _>>()?;
# Ok::<(), pdfrum::Error>(())
```

C: [`pdfrum-capi`](crates/pdfrum-capi). WebAssembly:
[`pdfrum-wasm`](crates/pdfrum-wasm).

## ISO 32000

An empty feature cell is always compiled in. `forms`, `edit` and `codecs-all`
are on by default.

| | | support | feature |
|---|:---:|:---:|---|
| File structure, objects, xref, incremental updates | §7 | yes | |
| Standard security handler, revisions 2–6 | §7.6 | yes | |
| Public-key security handlers (`Adobe.PubSec`) | §7.6.4 | no | |
| Flate, LZW, RunLength, ASCIIHex/85, DCT | §7.4 | yes | |
| CCITT, JBIG2, JPX | §7.4 | yes | `codecs-all` |
| Paths, colour spaces, functions, shadings 1–7, transparency | §8 | yes | |
| Type 1, TrueType, Type 0, Type 3, CID; encodings, ToUnicode | §9 | yes | |
| File attachments | §7.11 | yes | `edit` to write |
| Annotations, outlines, destinations | §12 | yes | |
| JavaScript actions | §12.6.4.4 | yes | `javascript` |
| AcroForm | §12.7 | yes | `forms` |
| Signatures, as written (unverified) | §12.8 | yes | |
| Tagged PDF, structure tree | §14.7 | yes | |
| Text extraction | §14.8 | yes | |
| PDF/A | ISO 19005 | yes | convert needs `edit` |

## Beyond the spec

| | support | feature |
|---|:---:|---|
| Host font fallback | yes | `system-fonts` (not on wasm32) |
| Markdown | yes | `markdown` |
| SVG export | yes | `svg` |
| Draw an SVG into a page | yes | `svg-ingest` |
| Extra CPU rasterizers | yes | `tinyskia`, `agg` |
| GPU rasterizer | yes | `vello-gpu` |
| `Pixmap` → PNG | yes | `png` |
| XFA | no | |
| Viewer (window, caret, chrome) | no | |

`default-features = false` is a parser and extractor. A render always names
its backend.

```toml
pdfrum = { version = "0.1", default-features = false, features = ["tinyskia", "codecs-all"] }
```

## Docs

| | |
|---|---|
| [docs.rs/pdfrum](https://docs.rs/pdfrum) | crate API |
| [CONTRIBUTING.md](CONTRIBUTING.md) | build, gate, board |
| [CHANGELOG.md](CHANGELOG.md) | what changed |
| [SECURITY.md](SECURITY.md) | vulnerability reports |

## Licence

Apache-2.0 or MIT, at your option. Contributions are dual-licensed the same
way.

No PDFium source is in the tree. Tested against its suite as an oracle —
**99% SSIM**. Upstream **data** that does travel with the repo
sits beside a `PROVENANCE.md`: Foxit fallback fonts
(`crates/pdfrum-font/fontdata/`, BSD-3-Clause), CJK CMaps and Unicode tables,
and a handful of test PDFs (BSD-3-Clause).
