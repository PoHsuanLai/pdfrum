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

Two footnotes outside the library tree:
- `libfuzzer-sys` (fuzz targets only, never in any published crate's tree)
  links LLVM's C++ libFuzzer runtime. It is the industry-standard fuzzing
  engine and worth keeping; if even dev-ring C++ is unwanted, the pure-Rust
  alternative is `fuzzcheck` (weaker ecosystem) — decision deferred, fuzz/
  is a separate non-published workspace either way.
- Future GPU `vello` speaks to OS graphics drivers via `wgpu` (system API
  calls, not vendored C) — a post-M8 decision.

## Policy

- Exact versions pinned at M0 scaffold; `Cargo.lock` committed.
- `cargo-deny` in CI: license allowlist (MIT / Apache-2.0 / BSD-3-Clause /
  Zlib — full audit is an M0 task), advisory DB, duplicate-version detection.
- Default features off wherever the crate allows; enable only what we use.
- Library crates depend only on rows marked **lib**; rows marked *tool/test*
  never appear in a library crate's `Cargo.toml`.

## Rendering & geometry

| Crate | Use | Why this one |
|---|---|---|
| `kurbo` **lib** | Paths, affines, rects — the geometry vocabulary of the whole workspace | Linebender; the ecosystem-standard 2D geometry crate; arcs/beziers/flattening done right |
| `peniko` **lib** | Brushes, gradients, blend modes, color | Shared vocabulary between our engine and both backends |
| `vello_cpu` **lib** | Primary rasterizer (`pdfrum-raster-vello`) | Modern sparse-strip CPU renderer; SIMD + multithreaded; native layers/masks/blends; "feature-rich, ready for production use cases" per Linebender, API still moving — pin exactly, wrap fully behind `RenderDevice` |
| `tiny-skia` **lib** | Cross-check rasterizer (`pdfrum-raster-tinyskia`) | Mature, deterministic Skia-CPU port (resvg's engine); Tier-C referee against vello_cpu |
| *(none)* | Parity rasterizer (`pdfrum-raster-exact`) | **Adds no dependency.** The analytic backend is written against `kurbo` and `peniko` alone — both already in this table — because the thing it exists to control is precisely what a third-party rasterizer decides for itself: how a partially covered pixel is quantised. Wrapping a fourth crate would reintroduce the question |
| `vello` *(future)* | GPU backend, post-M8 | Same peniko/kurbo types → near-free third backend |

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
| `ryu` **lib** | Shortest float formatting in the writer | Same guarantees class as the C++'s dragonbox |
| `smallvec` **lib** | Hot small collections (`CharItem` unicode, dash arrays) | Boring, ubiquitous |
| `thiserror` **lib** | Per-crate `Error` enums | The convention |
| `rayon` **lib** (facade only) | Parallel page rendering | Data-parallel fits; engine itself stays single-threaded per page (vello_cpu multithreads internally) |

## Tools & tests only

`anyhow`, `clap` (pdfrum-tool CLI), `png` (encode output; also decode goldens
in harness), `insta` (snapshots), `criterion` (benches, **pinned `=0.5.1`**),
`libfuzzer-sys` (fuzz targets), `cargo-deny` / `cargo-nextest` (CI tooling,
not deps).

`criterion` is held at 0.5 deliberately. From 0.6 it depends unconditionally
on `alloca`, which has a `cc` build-dependency and compiles C — which the
pure-Rust guarantee above forbids and `cargo-deny` rejects, failing
`scripts/ci.sh`. No feature flag avoids it (`alloca` is not optional). Unlike
`libfuzzer-sys`, this one is not worth an exception: `benches/` is inside the
workspace the ban walks, and 0.5 measures the same thing.
SSIM is hand-rolled in the harness (~60 lines) — the ratchet metric must
never shift under a dependency update.

## Performance ring (Phase 2) — admission by measurement

User ruling 2026-08-30: perf deps are welcome **only if measurably worth it**.
The protocol, binding for M12:

- **The bar:** a perf dependency lands only beside a committed A/B benchmark
  in docs/status/M12 — the same change implemented first as a tuned no-dep
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
| `fearless_simd` | **preferred SIMD dep** (already in-tree at =0.4.1 via vello_cpu) | Linebender, pure Rust, safe multiversioned runtime dispatch, heading to 1.0. Match vello_cpu's pinned version to avoid a duplicate; upgrading both is a coordinated bump. |
| `wide` | fallback SIMD (not in tree) | Compile-time-width model where fearless_simd's dispatch doesn't fit a loop. Needs the bar. |
| `bumpalo` | arena candidate (not in tree) | Bump arena for phase-scoped temporaries (content parse, page-graph build) AFTER RenderCaches buffer reuse is extended (no-dep first). Arena lifetimes must never leak into public types. Needs the bar. |
| `rustc-hash` | **pre-approved** (in-tree via subsetter) | FxHash for hot maps keyed by small ids (ObjRef store, glyph cache); SipHash is measurable overhead there. Record the number anyway. |
| `memchr` | candidate (not in tree) | SIMD byte-scan for the lexer and backwards keyword search. BurntSushi, runtime dispatch. Needs the bar. |
| `slotmap` | available (in-tree via fontdb) | Only if an id-arena store shape emerges; no current need. |

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
