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
  `scripts/check-no-wgpu.sh` (run by `scripts/ci.sh`) asserts that the `pdfrum`
  facade's default features, `pdfrum-tool`'s default features, and **every
  other workspace crate** reach none of `wgpu`/`wgpu-core`/`wgpu-hal`/
  `wgpu-types`/`vello`/`vello_encoding`/`vello_shaders` — plus a fourth
  assertion that the GPU crate *does* still depend on `vello`, so the other
  three cannot pass vacuously. Verified by negative control: one edge added
  from a leaf backend was caught on four crates at once (docs/status/M12c.md
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
compiler output that proves it: docs/status/M12c.md §1.

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
| `rayon` **lib** (facade only) | Parallel page rendering | Data-parallel fits; engine itself stays single-threaded per page (vello_cpu multithreads internally) |

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
| `boa_engine` **lib, feature-gated** | JavaScript engine (`pdfrum-form --features script`) — M15 | Pure Rust, 116 added crates, **zero `-sys`, zero `cc`/`cmake`/`bindgen`**, `cargo-deny` clean against the existing allowlist with no edit. 95.5% of test262; register VM; `RuntimeLimits` for loop/recursion/stack, which is a bound the C++ has no equivalent of. Pinned `=0.22.0`, `default-features = false`. **Reachable from no crate's default features**, asserted mechanically by `scripts/check-no-boa.sh`. Alternatives `rquickjs` and `deno_core` bind C and V8 and fail the purity rule outright |

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
1.91.0 for any build with `--features script`. Because the feature is
default-off, the floor applies **to the feature rather than to the
workspace** — but it must be written down in M13's declaration as a
per-feature MSRV rather than discovered by a consumer. Recorded here so M13
inherits a fact instead of a surprise.

### The isolation is a feature flag, which is the weaker mechanism

The GPU exemption is isolated by being *a crate nothing depends on*. The
engine is isolated by *a flag any workspace member can turn on*, and feature
unification means one member enabling it enables it for the whole build. That
is a real weakness of features and it is why `scripts/check-no-boa.sh` is not
optional. It asserts, on the same four-part shape as `check-no-wgpu.sh`: the
`pdfrum` facade's default features, `pdfrum-tool`'s default features, and
**every** workspace member (`conformance/` and `benches/` included) reach none
of `boa_engine`/`boa_ast`/`boa_parser`/`boa_gc`/`boa_interner`/`boa_string`/
`boa_macros` — plus the converse, that `pdfrum-form --features script` *does*
reach `boa_engine`, so the first three cannot pass vacuously. `scripts/ci.sh`
runs it.

**What the engine's limits do and do not bound** is a security fact and is
recorded in SPEC §10 with the measurement rather than here: boa's
`RuntimeLimits` bound loop iterations, recursion and stack — which V8 under
PDFium does not, at all — and bound neither heap growth nor regex
backtracking, which V8 under PDFium also does not. pdfrum is therefore
bounded where the oracle hangs and unbounded only where the oracle is too.
The measurement is `docs/status/data/v8probe/REPORT.md`.

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
| `fearless_simd` | **NOT ADMITTED** — measured 2026-08-30, M12 | Pre-approved as the preferred SIMD dep; the *measurement* declines it. The span compositor's inner loop was implemented against 0.4.1 (in-tree via vello_cpu, so zero new supply-chain cost) behind a default-off feature, proved byte-identical three ways *and* by 209 byte-identical rendered pages across all 44 corpus documents. A/B in docs/status/M12.md §3.9: **+1.4% geomean — slower** — and slower in every one of the four classes; best single document -1.0%, inside the noise band. **The cause is the workload, not the kernel:** 99.8% of the solid spans on `shading_axial_radial` are *one pixel* long, so a vector body reaches 0.0% of its pixels, and deleting the blend arithmetic entirely — the strict upper bound — buys only 5.4-17.5%. The gated code is deleted; the measurement is kept. Reopen only if the shading rasterizer stops decomposing into thousands of tiny fills. |
| `wide` | **NOT ADMITTED** — declined on the same evidence, M12 | The fallback for where fearless_simd's dispatch doesn't fit. It was never reached for: fearless_simd 0.4.1's API expressed the kernel fully, so the fallback's trigger never fired. And the negative result is not about the API — `wide`'s compile-time width does not change a span's length, and at the measured distribution a *wider* vector reaches strictly fewer pixels (docs/status/M12.md §3.9's W=4/8/16 table). Implementing it would re-measure the same workload. Reopens with the same condition as the row above. |
| `bumpalo` | **NOT ADMITTED** — measured 2026-08-31, M12b P2 | The pre-approval stands and the crate is clean: `3.20.3` has **zero transitive dependencies**, no build script, no C, `MIT OR Apache-2.0` — the tidiest candidate this project has evaluated. The *measurement* declines it, and the profile M12 §7 named as the prerequisite is what made the measurement possible. **The no-dep path came first, as this table requires, and it took the traffic away:** `RenderCaches` buffer reuse for the zero-area scan and the glyph placement, plus an `Arc` in the glyph cache to stop copying an outline nobody reads, cut the walk's allocations on the corpus's heaviest document from **3696 per render to 42** (23 650 KiB → 5.9 KiB) — and that deletion is worth **0.6%** of the render. A/B in docs/status/M12b-P2.md §8: against that tuned baseline the arena is **+265% / +121% / +97%** — *slower*, at all three of the object-count shapes the corpus presents, and slower than the untuned `Vec` too. The cause is structural rather than incidental: a bump allocator is fast at handing out memory and says nothing about touching it, and an arena cannot be reset per object (it is borrowed by the vector it is filling), so it trades a buffer that is warm in L1 for a fresh cold region on every object. Separately and independently disqualifying under PLAN.md's rule: the arena must live on `RenderCaches`, which is `pub caches` on the facade's `RenderSession`, so `Bump` puts a **lifetime in a public type**. The real finding is the split the profile gives: colour conversion ≤ 7.2%, allocation now 0.6%, and **interpretation is 60–90% of the engine half**. Reopen only on a workload where the walk's own allocation traffic exceeds a few percent of a render *after* `RenderCaches` reuse; the largest in the 44-document corpus is 0.6%. **One figure in this row was later corrected and the verdict was not** (M12b P3 §3, and docs/status/M12b.md §5): the "3696 → 42" census covered `walk.rs` and `clip.rs` only, and with `paint.rs` and `path.rs` counted the same render was making **21 421 allocations and 5.6 MiB**. That does not reopen anything — the A/B above was taken against the real renderer rather than against the census, the arena measured *slower* on every shape, and the public-type lifetime disqualifies the design independently — but a reopening argument must be made against the **full** site list, not against the partial one this row was first written from. |
| `rustc-hash` | **NOT ADMITTED** — measured 2026-08-30, M12 | The pre-approval stands; the *measurement* declines it. The no-dep baseline the protocol requires was built (`pdfrum_common::FxHasher`, rustc-hash's own algorithm, ~20 lines) and applied to both named call sites. A/B in docs/status/M12.md §3.7: every render number inside the noise band, in both directions; `open` moved 7 microseconds against renders of milliseconds. **Neither the dep nor the no-dep version clears the bar over doing nothing** — the maps are not hot enough for the hash function to be visible. The no-dep hasher is kept (one file, strictly less work, deterministic iteration order); the dependency is not added. Reopen if a profile ever shows object resolution dominating. |
| `memchr` | candidate (not in tree) — **unchanged through M12b**, P4 | SIMD byte-scan for the lexer and backwards keyword search. BurntSushi, runtime dispatch. Needs the bar — and needs an *input* first: M12 measured `open` at tens of microseconds on all 44 corpus documents against renders of milliseconds to a second, so nothing in the corpus makes lexing visible. Write the tuned scalar word-at-a-time scan, source a large file, then A/B. **M12b P4 left this row untouched deliberately** (docs/status/M12b.md §9): P1–P3 produced no large lexing input, and PLAN.md forbids synthesizing a giant PDF purely to justify a dependency — that is benchmarking the benchmark. The row is closed on a *missing input*, not on a failed measurement, which is why it carries no number. |
| `slotmap` | available (in-tree via fontdb) | Only if an id-arena store shape emerges; no current need. |

**M12's evidence-ranked queue** (docs/status/M12.md §7 has the loops and the
profile lines that nominate each):

1. ~~**`wide`, then `fearless_simd`** — the span compositor~~ — **RESOLVED,
   declined 2026-08-30.** Both rows above carry the number. The nomination
   rested on the profile charging 63.4% of a shading page to `fill_path`; that
   is per-fill overhead across 28368 fills, not compositing, and the spans
   themselves are one pixel long. Measured +1.4% geomean *slower*. See
   docs/status/M12.md §3.9, which also records the reopening condition: make the
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
   measured *slower* than that baseline on every shape. docs/status/M12b-P2.md
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
