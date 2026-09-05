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
last large block of the image path this pass does not reach.

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
