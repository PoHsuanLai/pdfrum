# Dependency Manifest — pdfrum

The **closed** set of external crates (STYLE.md §5). Adding, removing, or
swapping one is a `[spec]` change (SPEC.md §0). Verified against the ecosystem
2026-08-29.

## Pure-Rust guarantee

**No C or C++ is compiled into any library build, and no `-sys` binding crate
appears anywhere in the library dependency tree.** Every **lib** row below is
pure Rust — including the SIMD-heavy ones (`vello_cpu`, `zune-jpeg`, `moxcms`,
RustCrypto `aes` use Rust intrinsics, not C). "Pure Rust" here means no
foreign source compiled and no external C library linked; like every Rust
program, binaries still link the platform's libc/syscall layer through `std`
(e.g. `fontdb`'s optional `memmap2` wraps an mmap syscall — no C compiled).

Enforced mechanically in CI, not by trust: `cargo-deny` bans `cc`,
`pkg-config`, `cmake`, `bindgen`, and every `*-sys` crate in the workspace
graph — a transitive dep growing a C build script fails the build.

Three footnotes outside the library tree:
- `libfuzzer-sys` (fuzz targets only, never in any published crate's tree)
  links LLVM's C++ libFuzzer runtime. It is the industry-standard fuzzing
  engine and worth keeping; if even dev-ring C++ is unwanted, the pure-Rust
  alternative is `fuzzcheck` (weaker ecosystem) — decision deferred, fuzz/
  is a separate non-published workspace either way.
- `minicov` (M22 phase 4) build-depends on `cc`, and reaches the graph only as
  a dependency of `wasm-bindgen-test` — `crates/pdfrum-wasm`'s **dev**-
  dependency, the Node test harness. Measured 2026-09-05: `cargo tree
  --workspace --target all -e normal,build` finds no `cc` at all, so nothing
  that ships has a C compiler in its tree. `deny.toml` narrows the ban with
  `wrappers = ["minicov"]` rather than lifting it, so a second crate reaching
  for `cc` still fails. See "The web binding" below.
- GPU `vello` speaks to OS graphics drivers via `wgpu` (system API calls, not
  vendored C). No longer a post-M8 decision: promoted into M12c and **landed**
  as `pdfrum-raster-vello`. It is the single scoped exemption from the
  paragraph above, it is confined to that one crate, and the confinement is a
  CI check rather than a claim — see "The GPU exemption" under Rendering &
  geometry. Every *other* crate in this workspace remains pure Rust with no
  `-sys` crate in its tree.

## Policy

- Exact versions pinned at M0 scaffold; `Cargo.lock` committed.
- `cargo-deny` in CI: license allowlist (MIT / Apache-2.0 / BSD-3-Clause /
  Zlib — full audit is an M0 task), advisory DB, duplicate-version detection.
- Default features off wherever the crate allows; enable only what we use.
- Library crates depend only on rows marked **lib**; rows marked *tool/test*
  never appear in a library crate's `Cargo.toml`.

### Workspace-internal edges

This file's tables are the **external** set, and they are closed. Edges
*between* workspace members are governed instead by README.md's rule —
"dependencies flow strictly leaf-to-root; nothing below `pdfrum-page` may
depend on rendering" — and adding one is a `[spec]` change on the same
protocol (SPEC.md §0), recorded here so the graph has one place to read.

| Added | Edge | Why |
|---|---|---|
| 2026-09-02, WP1 step 4 | `pdfrum` → `pdfrum-crypt` | The facade names `pdfrum_crypt::Permissions` in `Document::permissions` and `Document::owner_permissions`, and re-exports it, so it depends on the crate that owns the type rather than laundering it through `pdfrum-parser`. **No external crate reaches the tree**: `pdfrum-crypt` was already there transitively through the parser, so the facade's `cargo tree` is unchanged. The alternative — `pdfrum-form` depending on `pdfrum-crypt` so the form's own `Permissions` could gain a `From` — was declined: it would pull `aes`, `cbc`, `cipher`, `md-5`, `sha1`, `sha2` and `unicode-normalization` into a crate about widget interaction, for two booleans. |
| 2026-09-05, M19 phase 4 | `pdfrum-markdown` → `pdfrum-common`, `pdfrum-object`, `pdfrum-page`, `pdfrum-text`, `pdfrum-doc`; `pdfrum` → `pdfrum-markdown` (optional, feature `markdown`) | A new leaf-to-root member beside `pdfrum-text` and `pdfrum-doc`: Markdown and layout-preserving text from a page graph. **No external crate reaches the tree.** The facade takes it as an optional dependency so `cargo add pdfrum` pays nothing for it unless asked. |

## Rendering & geometry

| Crate | Use | Why this one |
|---|---|---|
| `kurbo` **lib** | Paths, affines, rects — the geometry vocabulary of the whole workspace | Linebender; the ecosystem-standard 2D geometry crate; arcs/beziers/flattening done right |
| `peniko` **lib** | Brushes, gradients, blend modes, color | Shared vocabulary between our engine and both backends |
| `vello_cpu` **lib** | Primary rasterizer (`pdfrum-raster-vello-cpu`, renamed 2026-09-02, was `pdfrum-raster-vello`). **Since 2026-09-02 the facade's only rasterizer dependency** — see the note below | Modern sparse-strip CPU renderer; SIMD + multithreaded; native layers/masks/blends; "feature-rich, ready for production use cases" per Linebender, API still moving — pin exactly, wrap fully behind `RenderDevice` |
| `tiny-skia` **lib** | Cross-check rasterizer (`pdfrum-raster-tinyskia`) | Mature, deterministic Skia-CPU port (resvg's engine); Tier-C referee against vello_cpu |
| *(none)* | Parity rasterizer (`pdfrum-raster-agg`, renamed 2026-09-02, was `pdfrum-raster-exact`) | **Adds no dependency.** The analytic backend is written against `kurbo` and `peniko` alone — both already in this table — because the thing it exists to control is precisely what a third-party rasterizer decides for itself: how a partially covered pixel is quantised. Wrapping a fourth crate would reintroduce the question |
| `vello` **lib, scoped** | GPU backend (`pdfrum-raster-vello`, renamed 2026-09-02, was `pdfrum-raster-vello-gpu`) — M12c | Same peniko/kurbo types the engine already speaks. **The one exemption from the pure-Rust guarantee**; its blast radius and the checks that bound it are below. Pinned `=0.10.0`, which resolves `wgpu` **29** — see the version note |

### One rasterizer in the facade's tree (2026-09-02)

`cargo add pdfrum` used to compile **three** rasterizers whether or not the
caller used them, because `pdfrum::Backend` was an enum naming all three and
`Page::render` dispatched on it. That is now a trait bound rather than a
match: `Page::render_on` takes `&B where B: RasterBackend`, and `render` is
that call with `VelloCpuBackend`. `pdfrum-raster-tinyskia` and
`pdfrum-raster-agg` left the facade's `[dependencies]` and are the caller's
own — the same way `pdfrum-raster-vello` (GPU) always was.

Measured, `cargo tree -p pdfrum -e normal | grep -c pdfrum-raster`:

| | before | after |
|---|---|---|
| rasterizers in the facade's normal tree | **3** | **1** |

Nothing else about the closed set moves: all three crates stay workspace
members, stay built and tested, and the two that left the facade are its
dev-dependencies so `tests/facade.rs` still asserts every backend renders the
page the same size. The rule this obeys is the one this file already states —
a dependency is admitted for what it does, and a rasterizer nobody named was
doing nothing in an embedder's build.

### The GPU exemption: extent, and the checks that bound it

`wgpu` reaches the platform's graphics drivers, so `pdfrum-raster-vello` is
the only crate here that is not pure Rust to the syscall layer. PLAN.md §M12c
grants that on one argument — pdfrum's likely consumer is a wgpu-backed Rust
GUI that already holds an open device — and bounds it two ways. Both bounds are
mechanical, not prose:

- **Blast radius: one crate, nothing in the core ring.**
  `scripts/check-no-wgpu.nu` (run by `scripts/ci.nu`) asserts that the `pdfrum`
  facade's default features, `pdfrum-tool`'s default features, and **every
  other workspace crate** reach none of `wgpu`/`wgpu-core`/`wgpu-hal`/
  `wgpu-types`/`vello`/`vello_encoding`/`vello_shaders` — plus a fourth
  assertion that the GPU crate *does* still depend on `vello`, so the other
  three cannot pass vacuously. Verified by negative control: one edge added
  from a leaf backend was caught on four crates at once (the internal working notes
  §4.4). `cargo add pdfrum` puts no graphics driver in anyone's tree.
  *2026-09-02: this bound is unchanged and is now the only reason the facade
  cannot name this crate — `Page::render_on` means it does not have to. A
  caller who holds a `wgpu::Device` passes `VelloBackend::new(&device,
  &queue)?` straight to `render_on`, and the edge runs caller → GPU crate,
  never facade → GPU crate, so the check stays green. Asserted by
  `pdfrum-raster-vello`'s own `the_facade_renders_a_page_on_this_backend`
  test, which lives there rather than in the facade for exactly that reason.*
- **`cc` / `cmake` / `bindgen` are not relaxed at all** — none is in the tree,
  and no C is compiled. `pkg-config` stays banned except through two named
  `wrappers`, `wayland-sys` and `khronos-egl`, because `wgpu-hal` builds both
  with a feature that disables the probe (`dlopen` and `dynamic` respectively),
  so `pkg-config` sits in the graph and does nothing in it. `wrappers` rather
  than a blanket allow, because that reason is a property of the feature
  resolution and a third crate reaching for it should fail. Two permissive
  licences join the allowlist for the same subtree: `CC0-1.0` (`hexf-parse` via
  `naga`) and `ISC` (`libloading` via `ash`).
- **Excluded from the M13 publish set** (`publish = false`) unless it is
  genuinely ready.

**Version note, and it is not a footnote.** `vello 0.10.0` depends on `wgpu`
**29**, not the 30 PLAN.md originally predicted; there is no vello release
against 30. Since two `wgpu` majors in one tree are unrelated types, device
injection works today **only for a caller on `wgpu` 29** — and neither
`egui-wgpu` 0.36 (`^30.0`) nor `iced_wgpu` 0.14 (`^27.0`) is. So the
shared-device benefit that justifies this exemption is currently unavailable to
the frontends it was justified by, and arrives when vello bumps. Depend on
`wgpu` only *through* `vello` so our manifest cannot disagree with it, and use
the crate's re-exported `pdfrum_raster_vello::wgpu`. Full argument and the
compiler output that proves it: the internal working notes

## Fonts

| Crate | Use | Why |
|---|---|---|
| `skrifa` **lib** | Glyph outlines, metrics, charmaps | Google Fontations; pure Rust; upstream PDFium itself is adopting it — coverage validated by the oracle's own vendor |
| `read-fonts` **lib** | Low-level table access (CFF, cmap subtables skrifa doesn't surface) | Same project, same versioning train as skrifa |
| `fontdb` **lib** | System font discovery for substitution | Pure Rust, mature (resvg family); replaces `CFX_FolderFontInfo`/fontconfig |
| `subsetter` **lib** | Font subsetting in `pdfrum-edit` | Typst's; built precisely for PDF embedding, TrueType+CFF, `forbid(unsafe)`, one dependency. (Rejected: `hb-subset` C bindings; `fontcull`/klippa — absorbed into another project, unstable home) |

Type1/CFF charstrings: first-party `pdfrum-type1` remains the plan; evaluate
`hayro-postscript` during the font design brief before writing an interpreter.

## Codecs & color

| Crate | Use | Why |
|---|---|---|
| `miniz_oxide` **lib** | FlateDecode | Pure Rust, the ecosystem deflate; output cap enforced by our wrapper |
| `weezl` **lib** | LZWDecode | image-rs; supports the TIFF-variant LZW + EarlyChange that PDF uses; don't hand-write LZW |
| `zune-jpeg` **lib** | DCTDecode | Pure Rust at libjpeg-turbo speed (SIMD); image-rs is migrating to it, `jpeg-decoder` is in maintenance mode; fuzz-clean |
| `hayro-jpeg2000` **lib** | JPXDecode | **Pure-Rust JPEG 2000 decoder**, tested on 20k+ PDF-scraped images, passes most of the OpenJPEG suite; Apache-2.0/MIT. Erases our biggest planned port |
| `hayro-jbig2` **lib** | JBIG2Decode | Same project; erases the second big port |
| `hayro-ccitt` **lib** | CCITTFaxDecode | Same project, G3/G4; better PDF-tested than the `fax` crate |
| `moxcms` **lib** | ICC profiles / color management | Pure Rust, fast, CMYK/LAB/Gray/N-ink, ICC v4 class support. (Rejected: `lcms2` — C bindings; `qcms` — ICC v2 only) |

The hayro codec crates are wrapped behind our own thin entry points
(SPEC.md §12) so a first-party port stays a drop-in *fallback* if conformance
exposes gaps — no longer the default plan. We do **not** depend on
`hayro-syntax`/`hayro-interpret`/`hayro` itself (that's a competing engine,
not a codec).

## Crypto, text, numeric

| Crate | Use | Why |
|---|---|---|
| `aes`, `cbc`, `cipher` **lib** | AESV2/V3 | RustCrypto; audited, pure Rust |
| `md-5`, `sha1`, `sha2` **lib** | Key derivation, /R 2–6 | RustCrypto (RC4 is ~30 lines in-crate — no dep) |
| `unicode-bidi` **lib** | Bidi for text extraction | Servo's; replaces the entire ICU dependency |
| `unicode-normalization` **lib** | NFKC for the revision-6 password preparation in `pdfrum-crypt` (RFC 4013 SASLprep, step 2 of ISO 32000-2 §7.6.4.3.3) | unicode-rs; the ecosystem's normalisation crate. Admitted by the A31 `[spec]` change below — NFKC needs Unicode decomposition and composition tables that must not be hand-rolled or vendored. Pinned `=0.1.25` |
| `ryu` **lib** | Shortest float formatting in the writer | Same guarantees class as the C++'s dragonbox |
| `smallvec` **lib** | Hot small collections (`CharItem` unicode, dash arrays) | Boring, ubiquitous |
| `thiserror` **lib** | Per-crate `Error` enums | The convention |
| `rayon` **tests & examples only** (was "lib, facade only" until 2026-09-04: the library never called it) | Parallel page rendering, demonstrated | Data-parallel fits; engine itself stays single-threaded per page (vello_cpu multithreads internally) |

### `unicode-normalization`, measured — 2026-09-02 (`[spec]`, audit A31)

Same rule as `boa_engine` below: nothing is admitted on a claim. Measured on
this machine, `x86_64-unknown-linux-gnu`, at the pin.

| question | answer |
|---|---|
| crate and version | `unicode-normalization = "=0.1.25"`, pinned exactly |
| features | default (`std`); nothing added |
| crates added to `pdfrum-crypt`'s tree | **3** (31 → 34): itself, `tinyvec`, `tinyvec_macros` |
| crates added to the **workspace** tree | **1**. `tinyvec` and `tinyvec_macros` were already in `Cargo.lock`, reached through `fontdb` → `pdfrum-font`, so only `unicode-normalization` is new to the graph |
| `-sys` crates | **none** |
| `cc` / `cmake` / `pkg-config` / `bindgen` | **none** |
| build scripts | **none** — neither crate ships a `build.rs` |
| `cargo deny check` | **advisories ok, bans ok, licenses ok, sources ok**, with `deny.toml` untouched |
| licences | `unicode-normalization` `MIT OR Apache-2.0`; `tinyvec` `Zlib OR Apache-2.0 OR MIT`; `tinyvec_macros` `MIT OR Apache-2.0 OR Zlib`. Every `OR` resolves to an allowlisted branch |
| `unsafe` | the crate declares `#![deny(missing_docs, unsafe_code)]` itself |
| MSRV | **1.36**, well under the workspace floor |

The alternative was writing NFKC by hand or vendoring the Unicode
decomposition, composition and canonical-combining-class tables. STYLE.md §5's
"write the 30 lines" test is about *helpers*, and this is not one: NFKC is
several thousand table entries that change with every Unicode release, and a
hand-rolled copy would be wrong in exactly the places a password uses.

## Scripting (M15) — feature-gated, default-off

| Crate | Use | Why this one |
|---|---|---|
| `boa_engine` **lib, feature-gated** | JavaScript engine (`pdfrum-form --features javascript`, and `pdfrum --features javascript` / `pdfrum-tool --features javascript`, both of which only forward that flag and take no direct dependency on it) — M15 | Pure Rust, 116 added crates, **zero `-sys`, zero `cc`/`cmake`/`bindgen`**, `cargo-deny` clean against the existing allowlist with no edit. 95.5% of test262; register VM; `RuntimeLimits` for loop/recursion/stack, which is a bound the C++ has no equivalent of. Pinned `=0.22.0`, `default-features = false`. **Reachable from no crate's default features**, asserted mechanically by `scripts/check-no-boa.nu`. Alternatives `rquickjs` and `deno_core` bind C and V8 and fail the purity rule outright |

### The audit, run rather than promised — 2026-09-02

DEPS.md admits nothing on a claim. Every number below was measured on this
machine, `x86_64-unknown-linux-gnu`, at the pin above.

| question | answer |
|---|---|
| crate and version | `boa_engine = "=0.22.0"`, pinned exactly, per the policy |
| features | `default-features = false`, nothing enabled |
| crates added to `pdfrum-form`'s normal tree | **116** (50 → 166) |
| `-sys` crates | **none** |
| `cc` / `cmake` / `pkg-config` / `bindgen` | **none** |
| build scripts compiling C | **none** |
| `cargo deny --all-features check` | **advisories ok, bans ok, licenses ok, sources ok** |
| licence spread of the 116 | 54 `MIT OR Apache-2.0`, 18 `Unicode-3.0`, 17 `Apache-2.0 OR MIT`, 16 `MIT`, 10 `Unlicense OR MIT`, 2 `Zlib`, 2 `MIT/Apache-2.0`, 2 `BSD-3-Clause OR MIT OR Apache-2.0`, 1 `Apache-2.0/MIT`, 1 `Apache-2.0 OR BSL-1.0`. **Every `OR` resolves to an allowlisted branch**, so `deny.toml` needed no edit |
| boa crates in the **default** workspace tree | **zero**, verified by `cargo tree` over every member |
| MSRV | **1.91.0**, declared by every `boa_*` crate |

**`default-features = false` is load-bearing, not tidiness.** The three
defaults are `float16`, `xsum` and `temporal`; `temporal` alone drags
`icu_calendar`, `temporal_rs` and `timezone_provider` — an ICU tree, in a
project whose text section boasts that `unicode-bidi` "replaces the entire ICU
dependency" — for a `Temporal` API no PDF script has ever called. The dates
PDF scripts *do* use go through `util.printd`/`util.scand`, which are pure
functions in `pdfrum-script` and reach no engine at all.

**The MSRV is the one number that touches another milestone.** M13's exit
criterion is "MSRV declared and CI-checked", and `boa 0.22` sets a floor of
1.91.0 for any build with `--features javascript`. Because the feature is
default-off, the floor applies **to the feature rather than to the
workspace** — but it must be written down in M13's declaration as a
per-feature MSRV rather than discovered by a consumer. Recorded here so M13
inherits a fact instead of a surprise.

### The isolation is a feature flag, which is the weaker mechanism

The GPU exemption is isolated by being *a crate nothing depends on*. The
engine is isolated by *a flag any workspace member can turn on*, and feature
unification means one member enabling it enables it for the whole build. That
is a real weakness of features and it is why `scripts/check-no-boa.nu` is not
optional. It asserts, on the same shape as `check-no-wgpu.nu`: the `pdfrum`
facade's default features, `pdfrum-tool`'s default features, and **every**
workspace member (`conformance/` and `benches/` included) reach none of
`boa_engine`/`boa_ast`/`boa_parser`/`boa_gc`/`boa_interner`/`boa_string`/
`boa_macros` — plus **two** converses, so the first three cannot pass
vacuously: that `pdfrum-form --features javascript` *does* reach `boa_engine`, and
(added with WP12, when the facade grew a forwarding feature of its own) that
`pdfrum --features javascript` does too, since a facade feature that forwarded
nothing would be a feature in name only. `scripts/ci.nu` runs it.

**What the engine's limits do and do not bound** is a security fact and is
recorded in SPEC §10 with the measurement rather than here: boa's
`RuntimeLimits` bound loop iterations, recursion and stack — which V8 under
PDFium does not, at all — and bound neither heap growth nor regex
backtracking, which V8 under PDFium also does not. pdfrum is therefore
bounded where the oracle hangs and unbounded only where the oracle is too.
The measurement is a probe run against a V8-enabled build.

## Take what we need, not what a crate defaults to (2026-09-07)

User rule. A dependency is judged on the tree it actually pulls in *our*
configuration, not on its default feature set, and the burden is to check
before asserting either way.

The case that produced it: `usvg` was described here as heavy, on the
strength of `fontdb`, `harfrust`, `skrifa`, `memmap` and four `unicode-*`
crates appearing in its graph. Measured, every one of those is an optional
default feature. `usvg` with `--no-default-features` is **18 crates against
66** — an XML parser (`roxmltree`), a CSS selector engine (`simplecss`), an
SVG type parser (`svgtypes`), and small numeric utilities, two of which
(`kurbo`, `tiny-skia-path`) this workspace already carries. It still
resolves CSS classes, `use` references, nested transforms and shape-to-path
conversion, verified by a probe crate before the claim was made a second
time.

So: `default-features = false` and an explicit feature list is the default
posture for a new dependency, and a rejection on size has to name the
configuration it measured.

### The first application: `resvg`, M24 (2026-09-07)

`pdfrum-svg` writes SVG; the conformance board scores pixels and cannot
score SVG, so M24's proof is a round trip — render our own output with a
*second* engine and compare that to the oracle's PNG with the board's SSIM.
`resvg` is that second engine, and it is a **dev-dependency of
`crates/pdfrum-svg` alone**. It never enters any shipped tree:
`scripts/ci.nu`'s pure-Rust check walks `cargo tree -e normal`, which
excludes dev-dependencies, and the check passes unchanged.

| Crate | Version | Configuration | Why |
|---|---|---|---|
| `resvg` *test only* | `=0.47.0` | `default-features = false`, `features = ["raster-images"]` | The round-trip referee for `pdfrum-svg`. A second SVG rasterizer with its own antialiasing is the point: agreeing with ourselves proves nothing |

**The configuration, named as the rule requires.** The defaults are `text`,
`system-fonts`, `memmap-fonts` and `raster-images`. The first three exist to
*shape text*, and our SVG never asks for it — every glyph is already a filled
outline, so no font machinery is consulted. Dropping them drops `fontdb`,
`rustybuzz`, `ttf-parser`, `unicode-bidi`, `unicode-script`, `unicode-vo` and
`memmap2`. `raster-images` is the one feature kept, because it is what admits
`<image>` decoding and our images travel as PNG data URIs.

**Measured, in that configuration.** 27 crates in the subtree, of which 12 are
already in the lock — `kurbo`, `tiny-skia`, `tiny-skia-path`, `flate2`,
`bytemuck`, `byteorder-lite`, `log`, `memchr`, `roxmltree`, `strict-num`,
`weezl`, `zune-jpeg` — so the increment is **15 crates**: `base64`,
`color_quant`, `data-url`, `gif`, `image-webp`, `imagesize`, `pico-args`,
`quick-error`, `resvg`, `rgb`, `simplecss`, `siphasher`, `svgtypes`, `usvg`,
`xmlwriter`. All pure Rust; no `cc`, no `-sys`.

0.47.0 resolves `tiny-skia` **0.12.0**, the version this workspace already
pins, so no duplicate rasterizer enters the graph and `cargo deny`'s
duplicate-version check is unaffected.

**The shipped crate takes nothing new.** `pdfrum-svg`'s own dependencies are
`pdfrum-render`, `pdfrum-page`, `pdfrum-common`, `kurbo` and `peniko`, all
already here. base64 is written in-crate under STYLE.md §5. The PNG for the
embedded images is `pdfrum-render`'s `Pixmap::encode_png`, which this crate
reaches by enabling that crate's `png` feature — `png` is on the
tool-and-test-only list below for a *new* edge, but it is already an optional
library dependency of `pdfrum-render`, so using it here adds no crate to the
graph and keeps one encoder in the workspace rather than two.

## Rejected: a machine-learning runtime for `pdfrum-markdown` (2026-09-06)

Markdown extraction meets cases a model would answer better than a
heuristic: whether a borderless block of text is a table, whether a bold run
is a heading or emphasis, what a scanned page says. MinerU-rs answers them
with SLANet, a UNet line segmenter and OCR, and answers them well.

**Declined for this workspace**, on the user's ruling: `pdfrum-markdown`
represents what the document states and does not infer what it omits. The
crate is a few thousand lines with no dependency outside this workspace, and
that is the property that makes it usable in a build script or a serverless
function without weighing a runtime.

**Not behind a feature flag either.** §"The isolation is a feature flag,
which is the weaker mechanism" above already records why: an optional
dependency is still one the lockfile carries, `cargo deny` must rule on, the
pure-Rust check must special-case and CI must build both ways. `javascript`
earned that cost because running a document's own scripts is intrinsic to a
PDF engine. Inferring table structure from pixels is not intrinsic to
converting a PDF to Markdown.

**What this does not decline.** Most of what a document model recovers, a
PDF already states — ruling lines are path objects with exact coordinates,
text carries its positions, sizes and baselines. A model reading those from
a rasterized page is compensating for having rasterized it. We did not, so
we read them, which is not an approximation of the model but strictly better
input than the model gets. MinerU's *classical* stages — logical grid
inference from a set of lines, cell assembly, span recovery — are ordinary
geometry and portable on their own terms, with attribution.

The place for a model is a document-understanding project layered on this
crate, which is where MinerU already sits.

## Tools & tests only

`anyhow`, `clap` (pdfrum-tool and pdfrum-cli), `png` (encode output; also
decode goldens in harness), `serde` + `serde_json` (pdfrum-cli's `--json`
output only, `=1.0.229` / `=1.0.151`, both already in the lock through
other crates; no library crate depends on them), `criterion` (benches,
**pinned `=0.5.1`**), `libfuzzer-sys` (fuzz targets), `cargo-deny` /
`cargo-nextest` (CI tooling, not deps). `insta` was listed here once and
never used; the CLI's tests compare against plain expected-output files.

The command line's terminal crates, admitted 2026-09-05 with the pure-Rust
check (`cargo tree -e build`: no `cc`, no `-sys` beyond `linux-raw-sys`,
which is Rust syscall constants) and `cargo deny`:

| Crate | Version | Why |
|---|---|---|
| `crossterm` | `=0.29.0`, `default-features = false`, `events` + `windows` | Raw mode, the alternate screen, key events and the window size for `pdfrum view`. Brings `rustix` and its `linux-raw-sys` — generated syscall constants, `build = false`, Rust only — which `scripts/ci.nu`'s name check exempts by name. |
| `rpassword` | `=7.5.4` | The silent password prompt when an encrypted file is opened at a terminal without `--password`. |
| `sha2` | workspace | `pdfrum hash`: SHA-256 of the file and of the canonical object dump; the same crate `pdfrum-crypt` already uses. |
| `clap_complete` | `=4.6.9` | `pdfrum completions <shell>`, generated from the clap tree so it cannot drift from the parser. |
| `clap_mangen` | `=0.3.3` | `pdfrum manpage -o DIR`, one roff page per command from the same tree. Brings `roff`, Rust only. |

The command line's `javascript` feature (2026-09-05) is the facade's
`javascript` feature forwarded — `boa` and the ~137 crates behind it, all
pure Rust, already audited above — and adds no dependency of its own.
Off by default, for the binary size (+12 MB stripped) and because it runs
script out of untrusted documents.

Three the design named and this table does not: **`viuer`** — its
half-block path depends on `ansi_colours`, LGPL-3.0-or-later, which the
allowlist above refuses, so the three picture protocols (kitty, iTerm2,
half-blocks) are ~120 lines in `crates/pdfrum-cli/src/term.rs` with their
own base64; **`comfy-table`** and **`indicatif`** — tables and progress bars
only show at a terminal, which the tests never are, so they would have been
options with no reader the tests could see; plain aligned columns serve a
pipe and a person alike and are what the expected-output files pin.

`criterion` is held at 0.5 deliberately. From 0.6 it depends unconditionally
on `alloca`, which has a `cc` build-dependency and compiles C — which the
pure-Rust guarantee above forbids and `cargo-deny` rejects, failing
`scripts/ci.nu`. No feature flag avoids it (`alloca` is not optional). Unlike
`libfuzzer-sys`, this one is not worth an exception: `benches/` is inside the
workspace the ban walks, and 0.5 measures the same thing.
SSIM is hand-rolled in the harness (~60 lines) — the ratchet metric must
never shift under a dependency update.

## The C library's tools (M22 phase 2) — tools, not dependencies

`crates/pdfrum-capi` builds `libpdfrum.so`, `libpdfrum.a` and `pdfrum.h`, and
it adds **no external crate to any tree**: its only dependency is `pdfrum`, the
facade. The three things below are `~/.cargo/bin` binaries and a system
compiler. None of them appears in a `Cargo.toml`, none is in the lock file, and
none is compiled into anything shipped — which is why they are recorded here
rather than in a table above.

| Tool | Version | Install | Why, and what it is not |
|---|---|---|---|
| `cbindgen` | 0.29.4 | `cargo install cbindgen --locked` | Generates `crates/pdfrum-capi/include/pdfrum.h` from the Rust signatures, so the header cannot drift from the library. A **build-time generator run by a person**, not a build script: the header is committed, and `scripts/capi-header.nu check` fails CI when the committed one differs from a fresh generation. A contributor who never touches the C ABI never needs it. |
| `cargo-c` | 0.10.25 | `cargo install cargo-c --locked` | `cargo cinstall` lays down the library, the header and a `pdfrum.pc` for `pkg-config`, from the `[package.metadata.capi]` section. Packaging only; nothing in the repository's own build or test path calls it. |
| a C compiler (`cc`) | any C11 | the platform's | Compiles `crates/pdfrum-capi/ctest/test.c` against the header and links the built library. A **test-time** tool: `scripts/ci.nu` skips the stage with a printed note when `cc` is absent, exactly as it does for `cargo-deny`. |

The last row is the one worth being explicit about, because it looks like an
exception to the pure-Rust guarantee and is not. That guarantee is about what
the **library** compiles and links: `cargo tree -e build` over
`crates/pdfrum-capi` has no `cc`, no `cmake`, no `-sys`, and `scripts/ci.nu`'s
name check walks this crate with every other. The C program is a *consumer* of
the shipped library, the way any embedder would be, and the whole point of
compiling it in CI is that no Rust test can prove a header matches a library.
It found a real defect on its first run: `pdfrum_page` had been both a type and
a function, which C has one namespace for, and the accessor is now
`pdfrum_document_page`.

## The web binding (M22 phase 4)

`crates/pdfrum-wasm` builds the WebAssembly module and the JavaScript that
loads it (docs/design/wasm.md). It adds **three crates** to the workspace, all
from the `wasm-bindgen` project, all `MIT OR Apache-2.0`, and none of which
reaches any other crate's tree: nothing depends on `pdfrum-wasm`.

| Crate | Version | Ring | Why |
|---|---|---|---|
| `wasm-bindgen` | 0.2.128 | lib (`pdfrum-wasm` only) | The binding itself: the `#[wasm_bindgen]` attribute that makes a Rust type an exported JavaScript class, `JsValue`, `Clamped`, and the generator for the `.d.ts` that PLAN.md §M22.4 asks for. There is no alternative — it is the de-facto and effectively only Rust-to-JavaScript boundary, and the `wasm32-unknown-unknown` ABI it implements is what every wasm toolchain expects. Pure Rust; the "glue" it generates is JavaScript source, not compiled C. |
| `js-sys` | 0.3.105 | lib (`pdfrum-wasm` only) | Bindings to the JavaScript *standard library* — `Error` and `Reflect`, which the crate's one error conversion uses to build a thrown `Error` with a `.code` on it. wasm-bindgen's own companion crate, from the same repository and released in lockstep; it adds nothing to the tree that `wasm-bindgen` had not already put there. **Not a `-sys` crate in the usual sense** despite the name: the "foreign" side is the host JavaScript engine reached through wasm-bindgen's imports, there is no native library to bind, and the published crate has no `build.rs` at all. `scripts/ci.nu`'s pure-Rust check exempts it by name for that reason, beside `linux-raw-sys`. |
| `wasm-bindgen-test` | 0.3.78 | **dev** (`pdfrum-wasm` only) | The test harness. A `#[wasm_bindgen]` export does not exist until a JavaScript runtime instantiates the module, so no host-target test can see one; this is what lets `cargo test --target wasm32-unknown-unknown` run the binding under Node, and it is the only such harness. See the `cc` note below. |

### `cc`, and why the ban is scoped rather than lifted

`wasm-bindgen-test` depends unconditionally on **`minicov`** — a coverage
helper, with no feature that turns it off in 0.3.78 — which build-depends on
`cc`. `deny.toml` bans `cc` outright, as the pure-Rust guarantee requires.

Measured 2026-09-05, before the exception was written:

- `cargo tree --workspace --target all -e normal,build` finds **no `cc` at
  all**;
- `cargo tree -p pdfrum-wasm --target wasm32-unknown-unknown -e normal,build`
  finds none either.

It is reachable only through `-e dev`. So nothing that ships — no library, not
`libpdfrum`, and not the `.wasm` module a browser downloads — has a C compiler
anywhere in its tree, and the guarantee at the top of this file, which is about
what the **library** compiles and links, is untouched. This is the same shape
as the `libfuzzer-sys` footnote above: dev-ring only, outside every published
crate.

The ban is therefore **narrowed, not removed**: `deny.toml` carries
`{ crate = "cc", wrappers = ["minicov"], … }`, the mechanism M12c's
`pkg-config` exception already established, so a second crate reaching for `cc`
still fails the build — which is the point.

The alternative was to have no test harness for the WebAssembly binding at all.
An untested binding is the worse trade.

## The web binding's tools — tools, not dependencies

Three `~/.cargo/bin` binaries and a JavaScript runtime. None appears in a
`Cargo.toml`, none is in the lock file, and none is compiled into anything
shipped. `scripts/ci.nu` skips its WebAssembly stage with a printed note when
any is absent, exactly as it does for `cargo-deny` and the C compiler.

| Tool | Version | Install | Why, and what it is not |
|---|---|---|---|
| `wasm-bindgen-cli` | 0.2.128 | `cargo install wasm-bindgen-cli --locked` | Post-processes the `.wasm` cargo produces into the loadable module, the ES-module loader and `pdfrum.d.ts`. Its version **must match** the `wasm-bindgen` dependency above; a mismatch is a runtime error with a clear message. It also provides `wasm-bindgen-test-runner`, the harness `cargo test --target wasm32-unknown-unknown` invokes — named in `crates/pdfrum-wasm/.cargo/config.toml`. |
| `wasm-opt` | 0.116.1 | `cargo install wasm-opt --locked` | Binaryen's optimizer, `-Oz`, worth ~270 kB (5%) on the shipped module. The crate is a Rust wrapper around Binaryen's C++, which is why it is installed as a **binary and never depended on**: nothing in this repository's build or test path links it, and `cargo tree` never sees it. Optional — `scripts/wasm-package.nu --skip-opt` checks the budget against the unoptimized module, which is strictly harder to pass. |
| Node | 22.19 | the platform's | Runs the WebAssembly tests. A **test-time** runtime, on the same footing as the C compiler that builds `ctest/test.c`: it executes a consumer of the shipped module, and is no part of what the module is built from. |
| `wasm-pack` | — | — | **Tried and rejected.** It runs `wasm-opt` itself with a hardcoded `-O` and no feature flags, and that invocation *fails* on this module: rustc's `wasm32-unknown-unknown` emits bulk-memory and non-trapping float-to-int, which `wasm-opt`'s validator rejects unless told they are allowed. The flags cannot be supplied either — wasm-pack reads `[package.metadata.wasm-pack.profile.<name>]` only for `dev`, `release` and `profiling`, and this crate builds under the workspace's own `wasm` profile, under which it consults no metadata at all. `scripts/wasm-package.nu` calls the three underlying tools directly instead, which also lets it measure the module before and after the optimizer and enforce the size budget. Recorded here so the next person does not spend the afternoon rediscovering it. |

The `wasm32-unknown-unknown` **target** (`rustup target add
wasm32-unknown-unknown`) is a rustup component rather than a tool, and is the
one thing on this list that the facade already needed: `scripts/ci.nu` has
checked the facade against it since M22 phase 1.

## Benchmark peers (M21) — never in any tree of ours

The comparative benchmark, `benches/compare/`, links the competing engines
PLAN.md §M21 names so they can be measured beside pdfrum on one corpus
against one oracle. They are dependencies of **that crate only**, which is
its own workspace (an empty `[workspace]` table, the same severing `fuzz/`
uses), not a member of the root one — so `scripts/ci.nu`'s `cargo tree
--workspace` never sees them, `cargo add pdfrum` never gets them, and the
"Explicitly rejected" paragraph below still holds for every published crate.
Each peer sits behind a cargo feature of its own name; the default set is
the pure-Rust ones, and the two C baselines are `--features c-engines`.
`benches/compare/deny.toml` carries the root allowlist plus the exceptions
this table records, with comments; the root `deny.toml` is untouched.

| Crate | Version | Licence | C in the build | Why it is measured |
|---|---|---|---|---|
| `hayro` + `hayro-interpret` | `=0.7.1` / `=0.7.0` | Apache-2.0 OR MIT | none | The closest pure-Rust rasterizer, and the interpreter under it. `hayro-interpret` has no text API; the harness assembles text from its glyph stream and labels the row as such. |
| `pdf-extract` | `=0.12.0` | MIT | none | The most-downloaded pure-Rust text extractor (it drags a second `lopdf`, 0.42, and through it the unmaintained `ttf-parser` 0.25 — RUSTSEC-2026-0192, ignored in the benchmark `deny.toml` only). |
| `lopdf` | `=0.44.0` | MIT | none | The object-level reader/writer most Rust code uses; open + object walk, and its own `extract_text`. |
| `pdf` (pdf-rs) | `=0.10.0` | MIT | none | The typed object model; open + page walk. No text or render API (pdf-rs's renderer, `pdf-render` 1.0-beta, is a commercial-licence fork and is not measured). |
| `pdf_oxide` | `=0.3.77`, feature `rendering` | MIT OR Apache-2.0 | none | Claims "5x faster, 100% pass on 3830 files"; open, render, text. Its `rendering` feature brings `tiny-skia`, `fast_image_resize`, `fontdb` 0.23 and a second `hayro-jpeg2000`/`hayro-jbig2`. |
| `pdfium-render` | `=0.9.3` | MIT OR Apache-2.0 | **PDFium itself**, bound at runtime through `libloading`; nothing compiled, but the `libpdfium.so` it needs is C++. `bindgen` optional and off. | The C baseline that is also our oracle, through the wrapper a crates.io user gets. Needs a `libpdfium.so`: the run records "not run, needs libpdfium" without one. |
| `mupdf` + `mupdf-sys` | `=0.8.0`, `default-features = false`, `base14-fonts` | **AGPL-3.0** | **yes**: `mupdf-sys` compiles the vendored MuPDF with `cc` and generates bindings with `bindgen` (needs a clang). | The other C engine every comparison is asked about. AGPL is why it is a baseline in a table and could never be a dependency; benchmark-only exception in `benches/compare/deny.toml`. |

The harness itself uses the same plumbing crates the root workspace already
admits (`anyhow`, `clap` with `env`, `serde`, `serde_json`, `png`) at the same
pins, plus `kurbo` to read glyph positions out of hayro's `Device` callbacks.
`docs/benchmarks/README.md` is the method; the numbers are under
`docs/benchmarks/data/`.

## Performance ring (Phase 2) — admission by measurement

User ruling 2026-08-30: perf deps are welcome **only if measurably worth it**.
The protocol, binding for M12 and every performance milestone after it:

- **The bar:** a perf dependency lands only beside a committed A/B benchmark
  in the milestone's status doc — the same change implemented first as a tuned no-dep
  baseline (autovectorization, buffer reuse), then with the dep. It stays if
  it clears **>= 10% on at least one bench class or >= 5% on the geomean**
  over that tuned baseline; otherwise the no-dep version ships. The DEPS row
  for an admitted dep records its measured number — the "why" is a number.
- Crates already in the tree transitively carry zero new supply-chain cost;
  same bar, but ties break toward adoption.
- `forbid(unsafe_code)` stays intact everywhere: deps carry their own audited
  unsafe, we write none.

| Candidate | Status | Notes |
|---|---|---|
| `fearless_simd` | **NOT ADMITTED** — measured 2026-08-30, M12 | Pre-approved as the preferred SIMD dep; the *measurement* declines it. The span compositor's inner loop was implemented against 0.4.1 (in-tree via vello_cpu, so zero new supply-chain cost) behind a default-off feature, proved byte-identical three ways *and* by 209 byte-identical rendered pages across all 44 corpus documents. The A/B (internal notes §3.9): **+1.4% geomean — slower** — and slower in every one of the four classes; best single document -1.0%, inside the noise band. **The cause is the workload, not the kernel:** 99.8% of the solid spans on `shading_axial_radial` are *one pixel* long, so a vector body reaches 0.0% of its pixels, and deleting the blend arithmetic entirely — the strict upper bound — buys only 5.4-17.5%. The gated code is deleted; the measurement is kept. Reopen only if the shading rasterizer stops decomposing into thousands of tiny fills. |
| `wide` | **NOT ADMITTED** — declined on the same evidence, M12 | The fallback for where fearless_simd's dispatch doesn't fit. It was never reached for: fearless_simd 0.4.1's API expressed the kernel fully, so the fallback's trigger never fired. And the negative result is not about the API — `wide`'s compile-time width does not change a span's length, and at the measured distribution a *wider* vector reaches strictly fewer pixels (the internal working notes's W=4/8/16 table). Implementing it would re-measure the same workload. Reopens with the same condition as the row above. |
| `bumpalo` | **NOT ADMITTED** — measured 2026-08-31, M12b P2 | The pre-approval stands and the crate is clean: `3.20.3` has **zero transitive dependencies**, no build script, no C, `MIT OR Apache-2.0` — the tidiest candidate this project has evaluated. The *measurement* declines it, and the profile M12 §7 named as the prerequisite is what made the measurement possible. **The no-dep path came first, as this table requires, and it took the traffic away:** `RenderCaches` buffer reuse for the zero-area scan and the glyph placement, plus an `Arc` in the glyph cache to stop copying an outline nobody reads, cut the walk's allocations on the corpus's heaviest document from **3696 per render to 42** (23 650 KiB → 5.9 KiB) — and that deletion is worth **0.6%** of the render. The A/B (internal notes §8): against that tuned baseline the arena is **+265% / +121% / +97%** — *slower*, at all three of the object-count shapes the corpus presents, and slower than the untuned `Vec` too. The cause is structural rather than incidental: a bump allocator is fast at handing out memory and says nothing about touching it, and an arena cannot be reset per object (it is borrowed by the vector it is filling), so it trades a buffer that is warm in L1 for a fresh cold region on every object. Separately and independently disqualifying under PLAN.md's rule: the arena must live on `RenderCaches`, which is `pub caches` on the facade's `RenderSession`, so `Bump` puts a **lifetime in a public type**. The real finding is the split the profile gives: colour conversion ≤ 7.2%, allocation now 0.6%, and **interpretation is 60–90% of the engine half**. Reopen only on a workload where the walk's own allocation traffic exceeds a few percent of a render *after* `RenderCaches` reuse; the largest in the 44-document corpus is 0.6%. **One figure in this row was later corrected and the verdict was not** (M12b P3 §3, and the internal working notes): the "3696 → 42" census covered `walk.rs` and `clip.rs` only, and with `paint.rs` and `path.rs` counted the same render was making **21 421 allocations and 5.6 MiB**. That does not reopen anything — the A/B above was taken against the real renderer rather than against the census, the arena measured *slower* on every shape, and the public-type lifetime disqualifies the design independently — but a reopening argument must be made against the **full** site list, not against the partial one this row was first written from. |
| `rustc-hash` | **NOT ADMITTED** — measured 2026-08-30, M12 | The pre-approval stands; the *measurement* declines it. The no-dep baseline the protocol requires was built (`pdfrum_common::FxHasher`, rustc-hash's own algorithm, ~20 lines) and applied to both named call sites. The A/B (internal notes §3.7): every render number inside the noise band, in both directions; `open` moved 7 microseconds against renders of milliseconds. **Neither the dep nor the no-dep version clears the bar over doing nothing** — the maps are not hot enough for the hash function to be visible. The no-dep hasher is kept (one file, strictly less work, deterministic iteration order); the dependency is not added. Reopen if a profile ever shows object resolution dominating. |
| `memchr` | candidate (not in tree) — **unchanged through M12b**, P4 | SIMD byte-scan for the lexer and backwards keyword search. BurntSushi, runtime dispatch. Needs the bar — and needs an *input* first: M12 measured `open` at tens of microseconds on all 44 corpus documents against renders of milliseconds to a second, so nothing in the corpus makes lexing visible. Write the tuned scalar word-at-a-time scan, source a large file, then A/B. **M12b P4 left this row untouched deliberately**: P1–P3 produced no large lexing input, and PLAN.md forbids synthesizing a giant PDF purely to justify a dependency — that is benchmarking the benchmark. The row is closed on a *missing input*, not on a failed measurement, which is why it carries no number. |
| `slotmap` | available (in-tree via fontdb) | Only if an id-arena store shape emerges; no current need. |

**M12's evidence-ranked queue** (the internal working notes has the loops and the
profile lines that nominate each):

1. ~~**`wide`, then `fearless_simd`** — the span compositor~~ — **RESOLVED,
   declined 2026-08-30.** Both rows above carry the number. The nomination
   rested on the profile charging 63.4% of a shading page to `fill_path`; that
   is per-fill overhead across 28368 fills, not compositing, and the spans
   themselves are one pixel long. Measured +1.4% geomean *slower*. See
   the internal working notes, which also records the reopening condition: make the
   spans long first (a shading rasterizer that does not decompose into thousands
   of tiny fills), then ask about vector width.
2. ~~**`bumpalo`** — the page-graph walk~~ — **RESOLVED, declined 2026-08-31.**
   The row above carries the number. The prerequisite this entry named — a
   profile that separates allocation churn from colour conversion from
   interpretation — was built as in-walk instrumentation rather than as `perf`
   (`perf_event_paranoid` is still 4 and lowering it needs a root an agent must
   not take), and it answered the question: **allocation was 0.6%, colour
   conversion ≤ 7.2%, and interpretation is 60–90% of the engine half.** The
   no-dep buffer reuse this table demanded first removed 99% of the walk's
   allocation traffic and bought a few percent on four documents; the arena
   measured *slower* than that baseline on every shape. the internal working notes
   §4 has the split and §8 the A/B. What the entry got right is that the guess
   would have been wrong: an arena aimed at the engine half would have been
   aimed at the wrong third of it.
3. **`memchr`** — as the row above says: needs an input before it needs a bar.

**Rejected for the perf ring:** `std::simd` (nightly-only; we are stable),
`pulp` (new tree duplicating fearless_simd's role), raw `core::arch`
intrinsics (breaks `forbid(unsafe)` for no gain over the safe wrappers),
`mimalloc`/`jemalloc` global allocators (C — the pure-Rust guarantee is not
for sale for an allocator swap; no mature pure-Rust global allocator exists).

## Explicitly rejected

`image` (umbrella crate, drags codecs we replace), `freetype-rs` /
`harfbuzz-rs` / `lcms2` / `openjpeg-sys` (C bindings; for JPX decode diffing
the oracle's own `--save-rendered-images` output is the C reference, so no
FFI is ever needed), `qcms` (ICC v2 only),
`jpeg-decoder` (maintenance mode), `fax` (less PDF-tested than hayro-ccitt),
`lopdf` / `pdf-rs` / `hayro` engine crates (competing engines; we may read
them for ideas, never link them), `font-kit` (C deps), `icu4x` (only bidi
needed — `unicode-bidi` is lighter).
