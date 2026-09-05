# Render pass — the memset, and what the study's §6(a) list is actually worth

**Method:** M18's (`docs/status/M18.md`) — callgrind `Ir`, one named cause per
commit with the before/after on the same measurement, the conformance board
compared row by row, and nothing claimed that was not run.

**The measurement**, reused unchanged from `docs/design/mupdf-comparison.md`
§0 so the numbers are comparable with the study's: the **marginal warm
render**, `benches/compare`'s child at `--warm 4` minus `--warm 0` under
callgrind, divided by 4, anchored on the inclusive count of
`benches/compare/src/engines/mod.rs:run`.

```sh
cd benches/compare
CARGO_TARGET_DIR=<m21> cargo build --release --features c-engines
valgrind --tool=callgrind --callgrind-out-file=<out> \
  <m21>/release/pdfrum-compare child --engine pdfrum --op render \
  --file <pdf> --out x.png --warm {0,4}
callgrind_annotate --inclusive=yes <out>   # take `engines3run`
```

That binary links the facade from the workspace by path, so it is rebuilt
after every commit here.

**The baseline reproduces the study.** Taken before anything was touched:

| page | this pass | study §0 |
|---|---:|---:|
| `text_tcpdf_063` | 250,040,099 | 250.0 M |
| `vector_en_tem` | 155,442,848 | 155.5 M |
| `shading_tcpdf_058` | 258,398,392 | 258.4 M |
| `text_quick_start` | 211,165,810 | 211.2 M |
| `mixed_tcpdf_045` | 165,076,130 | 165.1 M |

Within a tenth of a percent on every page, so the study's table is the
before-column and this document continues it.

---

## 1. The memset — §7's open question, answered

The study left this open: *"the 204 M of `__memset_avx2_unaligned_erms` in
every one of our profiles is the leading suspect"* for why the wall ratio
exceeds the `Ir` ratio, *"but I did not take cachegrind or `perf` counters to
confirm it."*

The question is answerable without either. Callgrind with
`--separate-callers=2` attributes the `memset` to its callers directly.
Warm `vector_en_tem`, whole process, `Ir`:

| caller of `__memset_avx2_unaligned_erms` | `Ir` | share | ours? |
|---|---:|---:|---|
| `flate2` deflate, via the harness's `write_all` | 114,944,210 | 10.27% | no — **PNG encode, outside the engine seam** |
| `vello_common::Pixmap::new` ← `VelloCpuDevice::rasterize` | 43,474,465 | 3.88% | **yes** |
| `calloc` ← `__rust_alloc_zeroed` ← `Pixmap::filled` ← `new_target` | 36,298,202 | 3.24% | **yes** |
| `pdfrum-filters::decode_flate` | 3,937,951 | 0.35% | cold-path, cancels in the margin |
| the rest (read-fonts, miniz, skrifa, zune-jpeg, vello fine) | ~5.6 M | 0.5% | mixed |
| **total** | **204,426,290** | | |

That total matches the study's 204 M exactly, which confirms the same
profile is being read.

**The first row is the largest and is not ours to fix, and not in the
measurement either.** It is the harness compressing the output PNG, which
sits *outside* `engines::run` and therefore outside every `Ir` figure in the
study's marginal table. Its 115 M is real wall-clock cost in the published
benchmark's per-file time but it is not engine work. Naming it is most of
the answer to §7's question about the wall/`Ir` gap: a large part of the
"unexplained" memory traffic in a `compare` run is PNG deflate, not
rendering.

The two rows that *are* ours are the two commits below. Together they are
78.4 M of the 204 M, and both are pure waste — a buffer zeroed and then
completely overwritten.

### 1.1 The one lesson that shaped both fixes

The first attempt replaced both zero-fills with a scalar per-pixel loop that
wrote each pixel exactly once. It measured **worse**:

| page | baseline | scalar per-pixel loop |
|---|---:|---:|
| `text_tcpdf_063` | 250,040,099 | 251,744,161 |
| `vector_en_tem` | 155,442,848 | 156,934,073 |

Doing strictly less work in `Ir` terms is not the same as fewer
instructions: glibc's `memset` and `memcpy` move 32 bytes per instruction,
so a scalar loop that touches each of 8.7 MB once costs more instructions
than two AVX2 passes over the same bytes. **Removing a pass only pays if the
replacement is also bulk.** Both landed fixes therefore keep the write bulk,
using `extend_from_within` — log2(N) `memcpy`s that total one buffer's worth
of writes.

### 1.2 `Pixmap::filled` writes the buffer once (`a7b8efb`)

`crates/pdfrum-render/src/pixmap.rs`. `filled` was `new` (`vec![0; len]`,
one full `memset`) followed by `fill` (a second full pass writing the real
colour). `new_target` builds one of these per page, per transparency group,
per pattern cell and per soft mask.

Now the buffer is built once: seed a four-byte pixel, double with
`extend_from_within` until it fits, copy the partial remainder.

| page | before | after | |
|---|---:|---:|---:|
| `text_tcpdf_063` | 250,040,099 | 242,082,261 | −3.2% |
| `vector_en_tem` | 155,442,848 | 147,558,406 | −5.1% |
| `shading_tcpdf_058` | 258,398,392 | 252,354,724 | −2.3% |
| `text_quick_start` | 211,165,810 | 203,246,515 | −3.8% |
| `mixed_tcpdf_045` | 165,076,130 | 157,155,801 | −4.8% |

~7.9 M on every page regardless of content — the fingerprint of a fixed
per-pixel tax rather than a content-dependent one, which is what a
page-sized `memset` is.

Board: 1759 rows, **0 changed**.

### 1.3 The vello-side seed was written, measured, and dropped

The other 43.5 M — `vello_common::Pixmap::new` inside
`VelloCpuDevice::rasterize`, which allocates a zeroed page-sized buffer and
then `copy_from_slice`s over every byte of it from `base` — was implemented
the same way and **does not pay**.

The change was type-driven and pixel-clean. `VelloCpuDevice::base` became a
`Backdrop` enum, `Uniform(Color)` or `Pixels(Pixmap)`, so the seven of eight
target-creation sites that start from one colour never materialise a
page-sized `Pixmap` at all; `rasterize` built vello's buffer by doubling one
`PremulRgba8`. A test pinned the two constructions byte-for-byte across five
colours and six sizes including odd ones, and the **conformance board came
back 0 changed rows of 1759**, so this was measured on its merits, not
discarded on suspicion.

Marginal `Ir`, on top of §1.2:

| page | after §1.2 | with the vello seed | |
|---|---:|---:|---:|
| `text_tcpdf_063` | 242,082,261 | 232,389,624 | −4.0% |
| `vector_en_tem` | 147,558,406 | 137,905,630 | −6.5% |
| `shading_tcpdf_058` | 252,354,724 | **324,466,961** | **+28.6%** |
| `text_quick_start` | 203,246,515 | 193,625,744 | −4.7% |
| `mixed_tcpdf_045` | 157,155,801 | 147,499,301 | −6.1% |

Four pages gain ~10 M each; `shading_tcpdf_058` loses 72 M, and the sum over
the five pages is **+33.5 M — a net loss**. The shading figure was
re-measured from scratch and reproduced to within 600 `Ir`
(324,466,961 against 324,467,515), so it is deterministic, not load noise —
`Ir` is an instruction count and does not move with the box's load anyway.

**The cost is not in the new code.** It is inside `vello_cpu`'s own
`fine::rasterize_region`, which went 819.6 M → 1,024.4 M on that page for an
unchanged sequence of device calls, with `core::slice::specialize` (its bulk
fills and copies) going 97.6 M → 121.9 M alongside. The seeding function
itself costs 540 `Ir`. `shading_tcpdf_058` builds 27 render targets per
render where the other pages build one or two, so the effect scales with
target count.

**Why this is left unexplained rather than guessed at.** The mechanism was
not identified. `may_have_transparency` was the obvious suspect and was ruled
out — the flag is preserved exactly (`from_parts`, not
`from_parts_with_opacity`), and `vello_cpu`'s renderer and dispatchers never
read the target's copy of it; only gradient and image brushes carry one.
The remaining hypothesis, untested, is allocator provenance: `vec![0; n]` for
a multi-megabyte buffer is served by `calloc`, which for a fresh `mmap` region
returns kernel-zeroed pages **without executing a `memset` at all**, whereas
`Vec::with_capacity` plus a doubling write always touches every byte. On a
page that allocates and drops 27 large targets per render that difference
could plausibly dominate, and it would also mean part of the 43.5 M attributed
to `Pixmap::new` is not removable work in the first place. Confirming it needs
`cachegrind` or `perf` page-fault counters, which this pass did not take.

The change is reverted. The four-page win is real and is available to a
future pass that either understands the shading case or applies the same seed
only where the target count is small — but a change that is a net loss over
the study's own five pages does not land on the strength of four of them.

---

## 2. Resources reuse across renders — measured at 49 `Ir` per render, dropped

The study's §6(a) a2 names `Resources::new()` per `rasterize`
(`crates/pdfrum-raster-vello-cpu/src/lib.rs:174`) as discarding vello's own
image cache and atlas every render, and flags its size as **not separately
measured** (§7). It is now measured, and it is nothing:

| page | `Resources::new` inclusive, 5 renders | per render |
|---|---:|---:|
| `vector_en_tem` | 244 | **49** |
| `text_quick_start` | 244 | **49** |

Forty-nine instructions against a 155–211 M marginal render — 0.00003%. The
call count is 5, one per render, exactly as the study said; what the study
could not know without measuring is that the constructor allocates nothing
eagerly.

The reason the *consequence* is also nil is structural and already in our
favour. Vello's image cache would only earn its keep if the same image were
re-encoded into vello's atlas repeatedly, and it never is: our own
`RenderedImageCache` (`crates/pdfrum-render/src/imagecache.rs`) memoizes the
**rendered** pixmap upstream of the backend, keyed by `(ObjRef,
PixmapRequest)`, so on a warm render the image never reaches vello's cache at
all. This is the same asymmetry the study records in §2.4 and §4 item 4,
where we beat mupdf's `fz_scale_pixmap_cached` — and it is why fixing a2
would be fixing a cache that has nothing to hold.

**Dropped.** Nothing to do; the item is answered rather than deferred.

## 3. One colour conversion per fill — measured, and below the noise floor

The study's a3 (§3.4, §6(a)) reports `Argb::to_peniko()` called once per
device call rather than once per fill, so a fill-and-stroke object converts
twice and a degenerate path once per sub-path
(`crates/pdfrum-render/src/paint.rs:110,138,172-193`). The study calls it
"small — a handful of instructions against 44/px" and "free to fix".

Measured on the warm profiles, it is smaller than that:

| page | `resolve_argb` inclusive, 5 renders | calls | per render |
|---|---:|---:|---:|
| `vector_en_tem` | 53,285 | 220 | ~10,700 |
| `text_quick_start` | 177,850 | 715 | ~35,600 |

`to_peniko` itself does not appear as a call at all — it is
`peniko::Color::from_rgba8`, four `u8`-to-`f32` conversions, and it inlines
into its callers. The whole colour-resolution stage, conversions included, is
**0.007% of a marginal render on `vector_en_tem` and 0.017% on
`text_quick_start`**.

**Dropped.** A change here cannot be measured against this method's noise:
it would be indistinguishable from zero on every page, and the pass's rule is
that an item lands only with a before/after that shows it paid. The
observation in §3.4 stands as a correctness-neutral tidiness note, not a
performance item.

## 4. The evicting store — not reached, and why

The brief made item 4 conditional: a byte-budgeted LRU over the
never-evicting caches, *only* if items 1–3 leave it as the largest remaining
cost. They do not, and neither does it.

**What is actually largest after §1.2** is unchanged from the study's §1.1:
`vello_cpu`'s `F32Kernel::pack` and `unpack`, ~55 `Ir` per pixel per render,
about 120 M per A4/150 DPI render — half of `text_tcpdf_063`'s whole marginal
cost and three quarters of `vector_en_tem`'s. It is upstream code, carries
upstream's own `// TODO: SIMDify`, and is explicitly out of this pass's scope
(another agent measures the kernel choice). Nothing in this crate competes
with it.

**The store's own size on this measurement is zero**, and that is not a
surprise — the study says so directly (§6(a) a4: *"Saves: nothing on the
single-page warm benchmark (both engines warm)"*). Both caches are warm by
the second of five renders, so eviction policy cannot appear in a marginal
warm figure at all. Building an LRU and reporting "no change on five pages"
would be a change with no evidence behind it, which is the thing this method
exists to prevent.

**What the design would be, if multi-document throughput becomes the goal.**
Recorded here so the next pass does not re-derive it:

- *The duty is already split correctly.* `BuildContext` holds what depends on
  the document (colorspaces, functions, decoded images at 100 MiB, fonts in an
  `Arc<FontCache>` — `crates/pdfrum-page/src/build.rs:61-142`); `RenderCaches`
  holds what depends on the session (glyph outlines, glyph bitmaps at 16 MiB,
  rendered images at 64 MiB — `crates/pdfrum-render/src/ctx.rs:127-176`). The
  gap mupdf closes and we do not is that neither is shared *across documents*
  (`fitz/store.c:707-720`, one store per `fz_context`, 256 MiB).
- *Neither cache is unbounded.* Both are byte-budgeted and both degrade by
  **refusing to insert** rather than evicting, each with a written reason:
  `BitmapCache` because a page's glyph repertoire is small and hot, so the
  entries an LRU would evict are the ones about to be wanted
  (`glyph.rs:664-691`); `RenderedImageCache` because a single image larger
  than the whole budget must still be cached, being drawn once per render
  (`imagecache.rs:190-201`). Those reasons are sound *within one document*.
  They stop being sound across documents, which is the case an LRU is for —
  document B's glyphs never displace document A's, so a long-lived process
  serving many files fills the budget with the first few and runs uncached
  afterwards.
- *The shape.* One store, owned by whatever outlives a `Document` (the shape
  `compare throughput` measures), holding the budgeted caches behind a
  recency order; mupdf's scavenger frees the **largest** evictable item from
  the LRU tail and restarts, deliberately freeing as few blocks as possible
  (`fitz/store.c:795-877`), which is the right policy for entries whose sizes
  span four orders of magnitude — exactly the spread `BITMAP_CACHE_BUDGET`'s
  own comment describes.
- *Type-driven.* The budget is a newtype, not a `usize`; an entry's size is
  computed once at insertion and carried with it rather than recomputed; and
  the store hands out a handle that cannot outlive the entry, so "refuse to
  insert" and "evict" stop being two different return shapes at the call
  sites.
- *The precondition.* A benchmark that renders **many documents in one
  process** and reports the tail, which the marginal-warm-render method here
  cannot express. Without that number there is nothing to hold the design to.

**Not started.** The condition the brief set for it was not met.

## 5. Dead-code sweep

Nothing to remove. §1.2 replaced the body of one function and introduced no
new item; §1.3, §2 and §3 landed nothing, so they left nothing behind. The
one helper the dropped §1.3 work added (`uniform_vello_pixmap`) went with the
revert, and `clippy --workspace --all-targets -D warnings` is clean, which is
what would have caught an unused one.

The sweep is recorded as run and empty rather than skipped, because a pass
that lands one commit can still leave a stale helper behind, and confirming
it did not is the point.

---

## 6. Wall clock, and the board

**The board.** 1759 rows, compared row by row against main's reference
(`board-goldens-main.json`), for §1.2 as landed and for §1.3 before it was
dropped: **0 changed rows** in both cases. Nothing in this pass moved a pixel,
which was the standing requirement.

**Gates**, per commit: `cargo fmt --all --check`; `cargo clippy --workspace
--all-targets --features pdfrum-tool/javascript,pdfrum-cli/javascript -- -D
warnings`; `cargo nextest run` over the seven render/page/facade/backend
crates, 1355 passed; `cargo test --doc -p pdfrum-render -p pdfrum`;
`RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace`;
`nu scripts/api-snapshot.nu check` (green, no baseline change needed — the
one changed function is a private body); `cargo check -p pdfrum --target
wasm32-unknown-unknown`.

**Wall clock**, `pdfrum-compare run --label … --corpus ../corpus --engines
pdfrum`, pdfrum only, the 44-file corpus, before against after §1.2:

| | median | mean | p95 | total |
|---|---:|---:|---:|---:|
| before | 13.58 ms | 51.31 | 39.96 | 2257.6 |
| after | **12.95 ms** | 53.33 | 46.75 | 2346.7 |

**The load average is the caveat and it is a large one.** The box is shared;
the two runs were taken back to back at 9.46/13.66/14.84 and
11.34/15.73/15.58 with 32 cores, and the load moved between and during them.
The median improves by 4.6%, which agrees in sign and roughly in size with
the `Ir` figure, but the mean and p95 move the *other* way — driven by the
handful of very large files (`image_bug_583804` alone is 1.5–1.6 s) whose
timing is dominated by whatever else the box was doing. **`Ir` is the number
that decides in this document**, exactly as `docs/status/M18.md` and
`docs/status/font-cache.md` say; the wall figures are recorded with their
load, not leaned on.

---

## 7. What was found and not fixed

- **`F32Kernel::pack`/`unpack` remains the whole ball game** — ~120 M `Ir`
  per A4/150 DPI render, half to three quarters of a marginal render, with
  upstream's own `// TODO: SIMDify` on it. Out of scope here (another agent
  measures the kernel choice) and it is `vello_cpu`'s code, not ours. Nothing
  in this crate is within an order of magnitude of it.
- **The PNG deflate `memset`, 114.9 M**, the largest single `memset` caller
  in the profile. It is the *harness* encoding its output, outside the engine
  seam, so it never appeared in the study's marginal table and is not engine
  work — but it is real time in any published per-file wall figure that
  includes the encode. Worth knowing before anyone reads a `compare` wall
  number as an engine number.
- **The vello-side seed's shading regression (§1.3)** is unexplained. The
  work is written and pixel-clean; what is missing is why removing a
  page-sized `memset` makes `vello_cpu`'s own `rasterize_region` 25% dearer
  on a page with 27 targets per render. The `calloc`-versus-written-pages
  hypothesis is stated there and is testable with `cachegrind` or page-fault
  counters, which this pass did not take.
- **`Pixmap::filled`'s remaining allocation** is not free even now: it is one
  bulk write of a page-sized buffer per target, and the only way past it is a
  buffer reused across renders. That needs the `RasterBackend` trait — whose
  methods take `&self` on a unit struct — to grow somewhere to keep one,
  which is an API-shape change (STYLE.md §2b territory), not a hot-loop one,
  and it is not obviously a win: §1.3 is a caution that the allocator may
  already be giving us the zeroing for free.
