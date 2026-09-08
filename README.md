<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/PoHsuanLai/pdfrum/main/docs/assets/banner-dark.svg">
  <img src="https://raw.githubusercontent.com/PoHsuanLai/pdfrum/main/docs/assets/banner-light.svg" alt="pdfrum" width="100%">
</picture>

**A composable PDF library and CLI built in Rust.**

[![crates.io](https://img.shields.io/crates/v/pdfrum.svg)](https://crates.io/crates/pdfrum)
[![docs.rs](https://docs.rs/pdfrum/badge.svg)](https://docs.rs/pdfrum)
[![CI](https://github.com/PoHsuanLai/pdfrum/actions/workflows/ci.yml/badge.svg)](https://github.com/PoHsuanLai/pdfrum/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#licence)

pdfrum is a modern, thread-safe PDF library — modular stages you compose,
CPU or GPU backends, no `unsafe` — 99% agreement with PDFium on its suite.
For your UI, RAG pipeline, or any app that has to open a PDF file.

pdfrum is an independent project, not affiliated with or derived from
Google's PDFium, which it uses as a read-only conformance oracle.

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
- **Pixels** — name a rasterizer; defaults to [`vello_cpu`](https://crates.io/crates/vello_cpu)
- **A new file** — edit, stamp, merge, flatten, save full or incremental

A file a browser would open, we open, and tell you where it's broken.

## Command line

![Pdfrum CLI](https://raw.githubusercontent.com/PoHsuanLai/pdfrum/main/docs/assets/cli/pdfrum-cli.gif)

```sh
pdfrum preview gradients.pdf
pdfrum stamp text gradients.pdf DRAFT --angle 30 --opacity 0.4 --size 72 -o stamped.pdf
pdfrum preview stamped.pdf
pdfrum view stamped.pdf
pdfrum search ISO paper.pdf
```

The rest of the catalog — `info`, `pages`, `forms`, `render`,
`serve --stdio` / `--mcp` — is [`pdfrum-cli`](https://crates.io/crates/pdfrum-cli).

```sh
cargo install pdfrum-cli
```

## Attributes

| | |
|---|---|
| Safe | `unsafe` is forbidden in the library. (The C ABI is the one exception, and only inside `extern "C"` or a private helper of one). |
| Pure Rust | no C/C++ in a library build |
| Thread-safe | documents, pages and text are `Send + Sync`; a render target travels to the thread that rasterizes it |
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

C: [`pdfrum-capi`](https://github.com/PoHsuanLai/pdfrum/blob/main/crates/pdfrum-capi/README.md). WebAssembly:
[`pdfrum-wasm`](https://github.com/PoHsuanLai/pdfrum/blob/main/crates/pdfrum-wasm/README.md).

## ISO 32000

An empty feature cell is always compiled in. `forms`, `edit` and `codecs-all`
are on by default.

| | | support | feature |
|---|:---:|:---:|---|
| File structure, objects, xref, incremental updates | §7 | yes | |
| Standard security handler, revisions 2–6 | §7.6 | yes | |
| Public-key security handlers (`Adobe.PubSec`) | §7.6.4 | no | |
| Flate, LZW, RunLength, ASCIIHex/85, DCT | §7.4 | yes | |
| CCITT, JBIG2, JPX | §7.4 | yes | `codecs-all` (`jpeg2000`, `jbig2`, `ccitt`) |
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
| Host font fallback | yes | `system-fonts` ([`fontdb`](https://crates.io/crates/fontdb); not on wasm32) |
| Markdown | yes | `markdown` |
| SVG export | yes | `svg-export` |
| Draw an SVG into a page | yes | `svg-import` ([`usvg`](https://crates.io/crates/usvg)) |
| SVG `<text>` on import | yes | `svg-text` |
| Extra CPU rasterizers | yes | `tiny-skia` ([`tiny-skia`](https://crates.io/crates/tiny-skia)), `agg` |
| GPU rasterizer | yes | `vello-gpu` ([`vello`](https://crates.io/crates/vello)) |
| `Pixmap` → PNG | yes | `png` |
| Every published capability except GPU | yes | `full` |
| XFA | no | |
| Viewer (window, caret, chrome) | no | |

`default-features = false` is a parser and extractor. A render always names
its backend.

```toml
pdfrum = { version = "0.1", default-features = false, features = ["tiny-skia", "codecs-all"] }
```

## Alternatives

Columns are what the benchmark harness exercises on the same 44 files;
`edit` is whether the crate writes a file back.

| crate | C in the build | licence | render | text | edit |
|---|:---:|---|:---:|:---:|:---:|
| pdfrum | no | MIT/Apache-2.0 | yes | yes | yes |
| [`pdfium-render`](https://crates.io/crates/pdfium-render) | yes (`libpdfium.so`) | MIT/Apache-2.0 | yes | yes | yes |
| [`mupdf`](https://crates.io/crates/mupdf) | yes (vendored C) | AGPL-3.0 | yes | yes | yes |
| [`hayro`](https://crates.io/crates/hayro) | no | Apache-2.0/MIT | yes | via `hayro-interpret` | no |
| [`lopdf`](https://crates.io/crates/lopdf) | no | MIT | no | yes | yes |
| [`pdf-extract`](https://crates.io/crates/pdf-extract) | no | MIT | no | yes | no |
| [`pdf-rs`](https://crates.io/crates/pdf) | no | MIT | no | no | yes |

`pdfium-render` binds a prebuilt `libpdfium.so`; `mupdf` vendors C through
`bindgen` and `cc`. Both want a native toolchain or a shipped binary, and
MuPDF is AGPL-3.0, which a commercial user has to price in. The pure-Rust
peers cost nothing to build but stop earlier: `lopdf` and `pdf-rs` are
object-level toolkits that read and write a file without rendering it,
`pdf-extract` only pulls text, and `hayro` renders and — with a second
crate — extracts, but writes no file back. Where pdfrum loses is speed:
warm median render is slower than both `pdfium-render` and `mupdf`.
Numbers, method, and the files each engine loses on are in
[`docs/benchmarks/`](docs/benchmarks/).

## Licence

Apache-2.0 or MIT, at your option. Contributions are dual-licensed the same
way.
