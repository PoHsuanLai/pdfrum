<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
  <img src="docs/assets/banner-light.svg" alt="pdfrum" width="100%">
</picture>

**A composable PDF library built in Rust.**

Tested against [PDFium](https://pdfium.googlesource.com/pdfium/)'s own suite
as a read-only oracle — not a binding, and equally capable on the files that
suite covers.

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

## What you get

Open a file once. Ask it questions. Rendering is one of them.

- **Pages** — boxes, rotation, the object graph a page paints
- **Text** — reading order, search, selection, words with geometry
- **Images, fonts, attachments, signatures** — as stored, decoded when that
  is useful
- **Forms** — read values, fill them, run a live session
- **Outline, links, annotations, tagged structure**
- **Pixels** — name a rasterizer; defaults to `vello-cpu`
- **A new file** — edit, stamp, merge, flatten, save full or incremental

A damaged file that can be opened *is* opened. What was repaired is
`Document::diagnostics`, not an `Err`.

![pdfrum CLI: info, doctor, extract, search](docs/assets/cli/pdfrum-cli.gif)

```sh
pdfrum info report.pdf
pdfrum doctor damaged.pdf
pdfrum extract toc report.pdf
pdfrum search ISO paper.pdf
```

Pages also draw in the terminal (`preview`, `view`). The rest of the catalog
— `pages`, `forms`, `stamp`, `render`, `serve --stdio` / `--mcp` — is
[`pdfrum-cli`](crates/pdfrum-cli/README.md).

```sh
cargo install pdfrum-cli
```

## Attributes

| | |
|---|---|
| Safe | `unsafe` is forbidden in the library. The C ABI is the one exception, and only at `extern "C"`. |
| Pure Rust | no C/C++ in a library build |
| Thread-safe | every public type is `Send + Sync` |
| JavaScript | off. The `javascript` feature runs the document's own scripts; nothing they call reaches a socket, a file, or a process. |
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

What a `cargo add pdfrum` build does, and which feature turns the rest on.
`default-features = false` is a parser and extractor; a render always names
its backend.

| | ISO 32000-1 | default | feature |
|---|:---:|:---:|---|
| File structure, objects, xref, incremental updates | §7 | yes | |
| Standard encryption, revisions 2–6 | §7.6 | yes | |
| Flate, LZW, RunLength, ASCIIHex/85, JPEG | §7.4 | yes | |
| CCITT, JBIG2, JPEG 2000 | §7.4 | yes | `codecs-all` |
| Paths, colour spaces, functions, shadings 1–7, transparency | §8 | yes | |
| Type 1 / TrueType / Type 0 / Type 3 / CID, encodings, ToUnicode | §9 | yes | |
| Host font fallback | | yes | `system-fonts` (not on wasm32) |
| Annotations, outlines, destinations | §12 | yes | |
| AcroForm | §12.7 | yes | `forms` |
| Signatures, as written (unverified) | §12.8 | yes | |
| Text extraction, search, selection | §14.8 | yes | |
| Tagged structure tree | §14.8 | yes | |
| Edit, save, subset, attachments | §7.5.8 | yes | `edit` |
| PDF/A check | ISO 19005 | yes | |
| PDF/A convert | ISO 19005 | yes | `edit` |
| Document JavaScript | | off | `javascript` |
| Markdown | | off | `markdown` |
| SVG export | | off | `svg` |
| Draw an SVG into a page | | off | `svg-ingest` |
| Extra CPU rasterizers | | off | `tinyskia`, `agg` |
| GPU rasterizer | | off | `vello-gpu` |
| `Pixmap` → PNG | | off | `png` |
| XFA | | no | |
| Public-key encryption (`Adobe.PubSec`) | | no | |

```toml
pdfrum = { version = "0.1", default-features = false, features = ["tinyskia", "codecs-all"] }
```

## Against PDFium

Correctness is agreement with PDFium on its test files. Live board:
`conformance/scoreboard.json` (2026-09-06).

| | |
|---|---:|
| Files passing every tier | **1675 / 1759 (95.2%)** |
| Load without crashing | 100% |
| Page counts agree | 1757 / 1759 |
| Text pages byte-exact | 2020 / 2067 (97.7%) |
| Render SSIM ≥ 0.99 | 1632 / 1668 (97.8%) |

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

No PDFium source is in the tree. Upstream **data** that does travel with the
repo sits beside a `PROVENANCE.md`: Foxit fallback fonts
(`crates/pdfrum-font/fontdata/`, BSD-3-Clause), CJK CMaps and Unicode tables,
and a handful of test PDFs (BSD-3-Clause).
