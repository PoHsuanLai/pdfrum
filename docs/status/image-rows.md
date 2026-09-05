# Image rows: what the pass cost and what it moved

The pass that implements `docs/design/image-rows.md`, landed 2026-09-05 in
four steps plus a dead-code sweep. Every step is measured by `Ir` under
`valgrind --tool=callgrind` on `benches/src/bin/profile.rs` built `--release`
(`--op render --iterations 1`), on the guide
(`benches/corpus/text_quick_start.pdf`, vello-cpu) and on
`benches/corpus/vector_en_tem.pdf` (agg), and gated by the conformance board.

## The commits

| | Commit | What |
|---|---|---|
| 1 | `cd9fbdc` | `Weight`/`Taps` in fixed point; the `f64` weight path goes |
| 2 | `3fc710e` | `Row`/`Rows`/`Source`/`Converted`; `to_pixmap` on rows; the per-pixel samplers go |
| 3 | `524ddf1` | `Narrowed`/`Shortened`; `convert_and_reduce` fuses conversion and reduction |
| 4 | `334f277` | `Placement::{Exact, Filtered}`; an image already at device size is not resampled twice |
| — | `70629c8` | dead-code sweep |

## `Ir` — the guide (`text_quick_start.pdf`, vello-cpu)

| | PROGRAM TOTALS | Δ |
|---|---:|---:|
| baseline | 3,862,301,097 | — |
| step 1 | 3,826,200,753 | −36.1 M (−0.93%) |
| step 2 | 3,180,845,670 | −645.4 M (−16.9%) |
| step 3 | 2,845,865,594 | −335.0 M (−10.5%) |
| step 4 | 2,845,859,281 | −6 K (flat) |
| **total** | | **−1,016.4 M (−26.3%)** |

Inclusive cost of the functions the brief asked for, per step:

| | baseline | step 1 | step 2 | step 3 | step 4 |
|---|---:|---:|---:|---:|---:|
| `to_pixmap` | 1,303,117,382 | 1,303,119,731 | 657,765,438 | *(off path)* | *(off path)* |
| `reduce_to` | 849,936,998 | 813,444,527 | 813,457,927 | *(off path)* | *(off path)* |
| `sample_bytes` | 575,875,960 | 575,875,960 | *(deleted)* | — | — |
| `unpack` | 291,825,213 | 292,024,367 | 292,029,817 | 291,906,255 | 291,906,255 |
| vello `paint_f32` | 311,473,796 | 311,473,796 | 311,473,796 | 311,473,796 | 311,473,796 |
| `convert_and_reduce` | — | — | — | 1,147,339,181 | 1,147,339,181 |

At step 3 `to_pixmap` and `reduce_to` leave the image path entirely:
`convert_and_reduce`'s 1,147 M replaces the 1,471 M those two cost between
them. Both functions survive for their other callers — `to_pixmap` for the
unreduced arm of the cache miss and the stencil path, `reduce_to` for
`prescale` and the soft mask.

## `Ir` — `vector_en_tem.pdf`, agg

| | PROGRAM TOTALS | agg `draw_image` (incl.) |
|---|---:|---:|
| baseline | 227,152,218 | 19,132,860 |
| step 1 | 226,777,631 | 19,132,638 |
| step 2 | 222,369,507 | 19,132,685 |
| step 3 | 220,324,227 | 19,127,732 |
| step 4 | 220,318,203 | 19,127,732 |

**−6.83 M (−3.0%)** overall. This file is mostly vector work, so it moves
much less than the guide, which is what it is in the set to show.

## The board

Run with `conformance -- run --check-regressions`, `per_file` rows compared
against the previous step. 1759 files, 1537 pass, 222 fail at every step —
the totals do not move across the whole pass.

| Step | Changed rows | Pass↔fail moves |
|---|---|---|
| 1 | 28 | none |
| 2 | 0 | none |
| 3 | 0 | none |
| 4 | 1 | none |
| sweep | 0 | none |

### Step 1's 28 rows

The fixed-point tap table is not bit-identical to the `f64` one, and the
board says so. Twenty-two rows move their SSIM by about 1e-6 in both
directions; two lose bit-exactness (`corpus/pdfium/bug_880920.pdf` and
`corpus/pdfium/bug_898443.pdf`, max channel diff 0 → 1, both still passing).

In every case examined it is the **`f64` table that had drifted** — see
`cd9fbdc`'s message for the `7 -> 1`, `10 -> 2` and `5 -> 1` tables side by
side, and `the_exact_table_is_pinned_where_the_float_one_drifted`, which pins
all three by value. This is the project's standing rule (implement the
correct behaviour, cite both) applied to our own float error rather than to
PDFium's. Ruled acceptable by the user, who amended the gate for steps 1–3 to
"no row moves between pass and fail, and every changed row is explained by
the tap table".

### Step 4's one row

`corpus/pdfium/bug_86459.pdf` — ssim 0.999967 → 0.999999, max channel diff
2 → 1, still passing. It **improves**: the second resample was interpolating
pixels that were already the answer, so removing it removes an error.

No image on either profiled document lands on `Exact` (`placement_for` runs
660 times on the guide and takes the filtered arm every time), which is why
step 4's `Ir` is flat and vello's `paint_f32` stays at 311 M there. The
saving is real but lives on the other class of file, the one `bug_86459`
represents, where an image is placed on the pixel grid.

## What the design turned out to need that it did not say

**`Weight` is a `u32`, not the `u16` the design named.** `Weight::ONE` is
`1 << 16`, one past the top of a `u16`, and it is a genuine single-tap value
for a footprint clamped to the image edge. A `u16` would have forced a
`whole: bool` beside the slice — a flag encoding a state the weight should
carry. The design doc is corrected in place.

**`Unpacked` and `Depth` are not here.** They belong with making `unpack`
itself lazy, which is a public change to `ImageData`/`Pixels` reaching
`crates/pdfrum/src` and `crates/pdfrum-cli`, which this pass was not allowed
to touch. Written up as "Step 5 — lazy unpack" at the end of the design doc,
with the four consumers named by file and line. It is worth **292 M** — the
last large block of the image path this pass does not reach. *Landed
2026-09-06; see "Step 5 — lazy unpack" at the end of this document.*

**The backends' `draw_image` does not take `Placement`.** `draw_image` is
also how glyphs, soft masks, patterns and the LCD text fallback reach a
backend, and none of those has a reduction behind it; threading `Placement`
through the trait would make every one of them construct a variant that means
nothing for it, to move a decision already made on the render side. The seam
keeps its transform-and-quality signature. What the design was buying is
preserved: `Exact` carries no transform, so the only way to draw it is
`transform()` paired with a `quality()` that has already collapsed to
`Nearest`.

**Consecutive destination rows' tap runs overlap**, which a pull pipeline
gets wrong for free. `Shortened` holds exactly one row back; one is enough
because the runs advance monotonically and each is at least one row long. The
symptom was alpha 223 where it should have been 255, on every row but the
first — invisible in a spot check, and caught by
`the_fused_reduction_is_the_two_call_one`.

**A short buffer must keep its partial row.** The per-pixel path read every
component through `get(..).unwrap_or(0)`, so a truncated stream painted the
bytes it had and black past them. Dropping a partial row instead moves that
boundary on every damaged image in the corpus.

## The ratchet

`cargo run --release -p pdfrum-bench --bin ratchet -- check`, over all eleven
groups × 44 documents:

```
ratchet: 440 benchmarks measured, 440 in the baseline
  252 unchanged, 37 improved, 151 regressed, 0 new, 0 not run
```

**Every image row that moved, improved. No image row regressed** — the
regression list contains no `image_` entry at all. The largest movements:

| Row | Before | After | |
|---|---:|---:|---:|
| `save/image_image_bug_898443` | 2.763 ms | 563.560 us | −79.6% |
| `render-cold-vello-cpu/image_image_jbig2_1478366` | 163.944 ms | 129.038 ms | −21.3% |
| `render-cold-agg/image_image_bug_898443` | 527.062 ms | 423.454 ms | −19.7% |
| `render-cold-agg/image_image_jbig2_1478366` | 157.712 ms | 127.054 ms | −19.4% |
| `render-cold-tinyskia/image_image_jbig2_880920` | 119.011 ms | 97.913 ms | −17.7% |
| `render-cold-agg/image_image_jbig2_880920` | 118.402 ms | 97.563 ms | −17.6% |
| `render-cold-tinyskia/image_image_jbig2_1478366` | 153.705 ms | 128.237 ms | −16.6% |
| `render-cold-vello-cpu/image_image_bug_898443` | 518.582 ms | 434.259 ms | −16.3% |
| `render-cold-tinyskia/image_image_bug_898443` | 497.041 ms | 422.492 ms | −15.0% |
| `render-cold-vello-cpu/image_image_jbig2_880920` | 118.382 ms | 100.737 ms | −14.9% |
| `render-cold-agg/image_image_jpx_123` | 77.259 ms | 71.109 ms | −8.0% |
| `render-cold-agg/image_image_bug_718762` | 488.437 ms | 450.827 ms | −7.7% |
| `render-cold-tinyskia/image_image_jpx_123` | 67.120 ms | 62.361 ms | −7.1% |
| `render-cold-agg/image_image_bug_583804` | 433.333 ms | 402.813 ms | −7.0% |

The guide's own cold render improved on all three backends —
`render-cold-tinyskia/text_text_quick_start` −15.2%,
`render-cold-vello-cpu/text_text_quick_start` −14.4%,
`render-cold-agg/text_text_quick_start` −11.2% — which is the wall-clock
confirmation of the −26.3% `Ir`.

### The 151 regressed rows are not this change

**They were measured on a loaded box**, with other agents' builds and
benchmarks running concurrently, and they are noise. The evidence is that the
worst of them are in code this pass never touched:

| Row | | |
|---|---|---:|
| `open/image_image_bug_898443` | 221.437 us → 2.177 ms | **+883.3%** |
| `open/text_text_quick_start` | 459.779 us → 1.822 ms | +296.4% |
| `open/forms_forms_widgets_407` | 1.220 ms → 3.735 ms | +206.2% |

The `open` group is `pdfrum-parser`'s, and this pass changed six files, none
of them in that crate: `pdfrum-page/src/{lib.rs, image/mod.rs, image/rows.rs}`
and `pdfrum-render/src/{image.rs, stretch.rs, walk.rs}`. There is no path by
which a row-based image pipeline makes opening a file nine times slower, and
the remaining regressions are the same shape at smaller magnitudes — spread
evenly across groups, sizes and backends, with no relation to whether a
document contains an image.

`Ir` is the evidence for this pass and it is machine-independent: it says
−1,016 M on the guide, and the ratchet's image and cold-render rows agree in
direction and rough magnitude. **The ratchet should be re-run on a quiet box
before anything is concluded from the regression list, and `ratchet update`
was deliberately not run.**

## Gates

Every commit: `cargo fmt --all --check`; `cargo clippy --workspace
--all-targets -- -D warnings`; `cargo nextest run` over `pdfrum-page`,
`pdfrum-render`, `pdfrum`, and the three raster backends; `cargo test --doc`;
`RUSTDOCFLAGS='-D warnings' cargo doc --no-deps`; and `cargo check -p pdfrum
--target wasm32-unknown-unknown --no-default-features --features
vello-cpu,codecs-all,forms,edit`. The API baselines were regenerated at step
2 (three `Pixels` methods out, the row stages in) and at the sweep (eleven
lines out, none in).

## Step 5 — lazy unpack

Landed 2026-09-06, on a branch from `036b675`, in three commits. The step the
first pass deferred because it is a public change to `ImageData`/`Pixels`
reaching crates that pass was not allowed to touch. Measured on the same
basis, on a different box from steps 1–4, so its baseline is re-established
here rather than continued from the table above.

### The commits

| | Commit | What |
|---|---|---|
| 1 | `048b0c7` | `Depth`/`Packed`/`Unpacked`/`Samples`; `unpack` becomes a row stage |
| 2 | `4e30401` | dead-code sweep |
| 3 | `df77df2` | the packed row walk reads each input byte once |

### The shape

`ImageData::pixels: Pixels` becomes `ImageData::samples: Samples`, a
two-state enum:

```rust
pub enum Samples {
    Packed(Packed),   // still packed, walked by `Unpacked`
    Whole(Pixels),    // a codec's eight-bit output, a stencil, or a palette
}
```

Two states rather than one state and a flag: the arms hold different data,
and `Source::new` dispatches on them once per image and then runs a loop that
cannot be handed the other. `Samples::to_pixels` is the one function that
materializes a whole image; `Samples::{components, byte_size, is_stencil,
palette}` are what the consumers actually wanted, and none of them needs the
pixels.

`Packed` carries the filter chain's bytes, a `Depth`, a component count, a
pitch, and the `/Decode` mapping already collapsed into one byte table per
component — so unpacking a sample is a table lookup rather than a float
multiply-add, a clamp, a round and a cast. `ImageData::byte_size` reports the
packed bytes plus that table, which is what the render cache is keeping
alive and is smaller than the widened form it never builds.

### What could not go lazy

| | Why |
|---|---|
| `Indexed`, single-colorant tint | the sample is a position in a palette that has to travel with it |
| multi-colorant `DeviceN` | `translate_image_line` is per pixel over a tuple too wide to tabulate |
| a stencil | its bits are its representation |
| DCT, JPX, JBIG2 | the codec already handed back eight-bit samples |

`widen_whole` is what is left of the eager generic loop, and the
multi-colorant `DeviceN` path is its only caller.

### `Ir` — the guide (`text_quick_start.pdf`, vello-cpu)

| | PROGRAM TOTALS | Δ |
|---|---:|---:|
| before step 5 (`036b675`) | 2,996,669,062 | — |
| commit 1 | 2,860,191,322 | −136.5 M (−4.55%) |
| commit 3 | 2,796,162,834 | −64.0 M |
| **total** | | **−200.5 M (−6.69%)** |

Inclusive cost of the functions the brief asked for:

| | before | commit 1 | commit 3 |
|---|---:|---:|---:|
| `decode_image` | 618,369,217 | 338,033,708 | 335,414,673 |
| `unpack` | 292,021,191 | *(deleted)* | — |
| `Unpacked::next_row` | — | 93,896,894 | 46,763,274 |
| `convert_and_reduce` | 1,147,743,076 | 1,291,625,944 | 1,230,285,796 |
| `to_pixmap` | *(off the image path since step 3)* | | |

`convert_and_reduce` *rises*, and that is the change working: the widening
moved inside the pull, so the row stage it drives is now counted under it.
`decode_image` giving up 283 M is the figure the step is about, and the
200.5 M program total is what is left once the work that genuinely still has
to happen is done in the right place.

**The 292 M eager pass is 47 M of row work.** The widening still happens; it
happens once per sample that is actually pulled, out of a table, with no
full-size buffer and no per-row allocation behind it.

### `Ir` — `vector_en_tem.pdf`, agg

| | PROGRAM TOTALS |
|---|---:|
| before step 5 | 217,011,601 |
| commit 3 | 216,891,439 |

−120 K. This file is vector work with almost no image in it, which is what it
is in the set to show.

### The board

Run from the worktree, own target dir, `--check-regressions`, `per_file`
compared against `board-imgrows-main.json`. 1759 files, 1537 pass, 222 fail —
the same totals as every step of the first pass.

| After | Changed rows | Pass↔fail moves |
|---|---|---|
| commit 1 | **0** | none |
| commit 2 (sweep) | **0** | none |
| commit 3 | **0** | none |

Byte-identical at every commit, which is the gate for a step that moves work
and changes no arithmetic. `--check-regressions` names
`resources/javascript/util_printd.in` and `util_scand.in` at every run,
including on an unmodified tree: both are date formatting and neither is in a
changed row.

### The ratchet

`cargo run --release -p pdfrum-bench --bin ratchet -- check`, not `update`:

```
ratchet: 225 benchmarks measured, 440 in the baseline
  57 unchanged, 138 improved, 30 regressed, 0 new, 215 not run
```

Twenty-seven image rows improved. The `build` group — which is the decode
itself, and so is where the eager unpack lived — moved most:

| Row | Before | After | |
|---|---:|---:|---:|
| `build/image_image_ccitt_3bigpreview` | 27.042 ms | 2.987 ms | −89.0% |
| `build/image_image_jbig2_1478366` | 60.762 ms | 19.758 ms | −67.5% |
| `build/image_image_bug_898443` | 267.291 ms | 155.850 ms | −41.7% |
| `build/image_image_bug_583804` | 199.558 ms | 125.152 ms | −37.3% |
| `render-cold-vello-cpu/image_image_jbig2_1478366` | 163.944 ms | 108.244 ms | −34.0% |
| `render-cold-agg/image_image_jbig2_1478366` | 157.712 ms | 106.297 ms | −32.6% |
| `render-cold-tinyskia/image_image_jbig2_1478366` | 153.705 ms | 106.129 ms | −31.0% |
| `build/image_image_jbig2_880920` | 56.863 ms | 41.044 ms | −27.8% |
| `render-cold-vello-cpu/image_image_bug_898443` | 518.582 ms | 381.500 ms | −26.4% |
| `render-cold-agg/image_image_bug_898443` | 527.062 ms | 388.203 ms | −26.3% |
| `render-cold-tinyskia/image_image_bug_898443` | 497.041 ms | 374.505 ms | −24.7% |
| `render-cold-agg/image_image_jbig2_880920` | 118.402 ms | 91.553 ms | −22.7% |
| `render-cold-vello-cpu/image_image_bug_718762` | 501.572 ms | 450.418 ms | −10.2% |
| `render-cold-agg/image_image_jpx_123` | 77.259 ms | 70.320 ms | −9.0% |
| `render-cold-agg/image_image_bug_583804` | 433.333 ms | 398.188 ms | −8.1% |
| `build/image_image_en_fqa` | 75.962 ms | 72.112 ms | −5.1% |

Eight image rows regressed. Five are `render-warm-agg`, which reuses the
cached pixmap and never re-decodes, at magnitudes (+148%, +124%, +82%) that
no decode-side change can produce. The two `build` rows were checked against
`Ir`, which is machine-independent:

| Row | Ratchet | `Ir`, before → after | |
|---|---|---|---|
| `build/image_image_ccitt_transfer` | +4.8% | 407,909,889 → 404,696,406 | **−3.2 M** |
| `build/image_image_bug_718762` | +8.5% | 9,239,549,466 → 9,240,739,043 | +1.19 M on 9.24 G (+0.013%) |

Both are cleared: the first improves, and the second is a CMYK JPEG that goes
through `decode_dct` and never reaches the packed path at all.

The other 22 regressions are shading, forms, text, vector and mixed rows in
code this step did not touch — the same shape, and several of the same rows,
the first pass documented. **The box is shared; `Ir` is the evidence, and
`ratchet update` was deliberately not run.**

### The regression the ratchet caught, and what it was

Worth recording, because it is the ratchet earning its place. The first cut
of `Unpacked::next_row` put `build/image_image_ccitt_transfer` at +4.8%, and
`Ir` confirmed a real +6.6 M on a file where every other function was
identical: `unpack`'s 1.88 M self cost had become `next_row`'s 6.73 M.

Two per-sample costs the eager loop did not have:

- `i % components`, an integer division on every sample. On a one-component
  one-bit image that division was the larger half of the loop body.
- `scanline::get_bits` per sample at the sub-byte depths, which re-derives
  the byte index, the shift and the mask each time — for a one-bit image,
  eight times per input byte.

Commit 3 makes the component index a counter that wraps, carrying its own
offset into the decode table, and walks the *byte* at the sub-byte depths,
shifting down through the samples packed into it. Each input byte is read
once. `Unpacked::next_row` fell 93.9 M → 46.8 M on the guide, and the
regression became a −3.2 M win.

### What the design got wrong

**`crates/pdfrum-cli/src/cmd/pages.rs:593`–`652` is not a consumer.** The
design named it as one of the four. That file declares a `Pixels` of its own,
with `Jpeg` and `Raw` variants, describing what an image *file* becomes on
the way into a save; it never sees `pdfrum_page::Pixels` and is untouched by
this step. The real consumers were three, and one of them —
`crates/pdfrum-render/src/imagecache.rs:102`'s stencil check — the design did
not name.

**`Source::at_row` cannot seek on a packed source.** It existed so a test
could reach one pixel without walking quadratically. A packed row comes out
of a bit walk that has no index to skip to, so that arm pulls forward
instead. Only tests reach it, and only for small `y`.

**A `Packed` constructor cannot take a `DecodeMap`.** `DecodeMap` is
`pub(crate)`, and `Packed::new` has to be callable from `pdfrum-render`'s
tests, so the public constructor takes the `&ColorSpace` and the `/Decode`
array — both already public — and builds the mapping itself. `with_map` is
the crate-internal one the image ladder uses, which already has the mapping
in hand.

### Gates

Every commit: `cargo fmt --all --check`; `cargo clippy --workspace
--all-targets --features pdfrum-tool/javascript,pdfrum-cli/javascript --
-D warnings`; `cargo nextest run` over `pdfrum-page`, `pdfrum-render`,
`pdfrum`, `pdfrum-cli`, `pdfrum-capi`, `pdfrum-wasm` — 1266 passed, 1
skipped; `cargo test --doc -p pdfrum -p pdfrum-page`; `RUSTDOCFLAGS='-D
warnings' cargo doc --no-deps --workspace`; `nu scripts/api-snapshot.nu
check`; and `cargo check -p pdfrum --target wasm32-unknown-unknown
--no-default-features --features
vello-cpu,codecs-all,forms,edit,markdown`. Once at the end,
`cd crates/pdfrum-wasm && cargo test --target wasm32-unknown-unknown` — 13
passed.

The API baseline gains `Depth`, `Packed`, `Unpacked` and `Samples`;
`ImageData::pixels` becomes `ImageData::samples`; `Source::new` takes
`&Samples`. The sweep took `Depth::max_raw` back out. **`pdfrum`'s own
surface does not move** — the facade never exposed `Pixels` — so
`pdfrum-capi` and `pdfrum-wasm`, which consume the facade's image API, are
unchanged.

### Step 5 follow-up — the warm regression, and what it actually was

Landed 2026-09-06. The step's benchmark run recorded a real regression:
`image_bug_583804`'s `render-warm-*` doubled on all three backends while
every `render-cold-*` row on the same file improved
(`docs/status/M13-perf-baseline.md` §25.2). The write-up there blamed the
deleted `Pixels::Whole` intermediate and the per-render row walk. It was
neither.

The pass changed what the **decoded-image cache holds**. Widened, this file's
samples were 60,023,187 bytes and fit `pdfrum_page::MAX_BYTES`; packed, they
are 120,242,982 — sixteen bits per component, so leaving them packed made the
cached form *twice as large*, not smaller. Over the budget, the eviction pass
dropped the entry in the same `insert` that added it, and every page rebuild
re-ran the JPEG decode. `render-warm-*` rebuilds its `Page` inside the timed
closure, so it paid that decode per iteration: 94.713 ms against 0.009 ms
before the pass, measured on `himmel`. `benches/src/bin/profile --warm`
hoists `prepare` out and never repeated it, which is why the regression was
invisible to the profiler and to callgrind.

The fix is in `crates/pdfrum-page/src/image/cache.rs`: the byte budget keeps
its last entry however large, so an image bigger than the whole budget is
cached rather than inserted-and-evicted. `RenderedImageCache` downstream
already had the rule. Warm returns to 192.02 ms (from 295.96) and cold to
425.96 ms (from 429.08), both on `himmel` at load < 0.1; peak RSS at 150 DPI
is unchanged.

The lesson for the design doc's ledger: `ImageData::byte_size` reporting the
*packed* bytes is honest about what is held, but "smaller than the widened
form it never builds" is only true below eight bits per component. At sixteen
it is larger, and any budget keyed on it has to tolerate that.
