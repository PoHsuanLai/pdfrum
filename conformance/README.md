# The conformance harness

Four subcommands over the oracle checkout and the golden store:

```
conformance generate-goldens   # run pdfium_test, record its answers
conformance run                # Tier A (byte-exact dumps) + Tier B (pixels)
conformance run --check-regressions conformance/scoreboard.json
conformance triage             # cluster the scoreboard's failures
conformance tier-c             # the two rasterizers against each other
```

`--tool` names the `pdfrum-tool` binary and defaults to
`target/release/pdfrum-tool` **under the repository root**, which is not where
a workspace with a redirected `CARGO_TARGET_DIR` puts it. When `tier-c`
reports `0 files compared`, that is the reason; pass `--tool` explicitly.

`--limit N` truncates the corpus listing to its first `N` entries. It is a
smoke-test switch, not a filter: there is no way to select a named file.

## Tier C's edge rule

Tier C does not compare against the oracle. It renders each page under both
rasterizers and asks whether the **engine** decided the pixels, by splitting
them into two populations (`docs/design/pdfrum-render.md` §6.3):

- **Interior** pixels are the engine's. They must match within
  `INTERIOR_TOLERANCE` (1 count), and any interior difference is a **hard
  failure** at any count and any size. No budget covers it.
- **Edge** pixels are the rasterizers'. `tiny-skia` supersamples and
  `vello_cpu` computes analytic area, so they integrate the same coverage ramp
  differently; these are allowed `EDGE_TOLERANCE` (8 counts), and the share of
  them that differs is *reported rather than gated*.

The brief derives the edge mask from a device-call trace the engine does not
yet emit, so the harness uses a neighbourhood proxy: a pixel is an edge when
one of its eight neighbours is unlike it **within the same image**.

### The rule change (2026-08-29)

The proxy previously stopped there. The brief's mask is
`dilate(edges, 1px)`, and omitting the dilation left a blind spot: a tiling
pattern's cell is rasterized once, antialiased, and then *blitted* at every
tile position, so the cell's own coverage difference arrives a pixel in from
any boundary the page has. In a locally flat neighbourhood it scored as
interior — a hard failure charged to the engine for a difference the
rasterizers are entitled to.

The mask is now dilated, **seeded from the intersection of the two per-image
boundary masks rather than their union.** That distinction is the rule:

- Growing the *union* would be worse than not growing at all. Wherever the
  images differ, the differing region has a boundary in one of them — its own
  rim — so dilating the union rolls that rim inward over the difference and
  excuses it. The metric would absorb precisely what it exists to detect.
- Growing from the *intersection* anchors the dilation to structure both
  backends independently found. It reaches a coverage difference beside real
  geometry, and it can never derive a difference's licence from that
  difference itself.

A divergence in open space therefore stays a hard failure at any size. That
property is pinned by `tierc::tests::a_whole_page_colour_divergence_survives_dilation`
(a page one backend fills red and the other white) and
`dilation_does_not_reach_across_open_space`; the recovered case is pinned by
`a_blit_seam_one_pixel_off_a_boundary_is_an_edge`.

The radius stays at the brief's 1px. Radius 2 was measured over the full
store: it clears no additional file and moves 32 files under the soft edge
budget purely by enlarging their denominator.

### What the change did, and what it left

Hard failures over the 1628-file store: **13 → 11**. The two cleared
(`ch_9_android.pdf`, `zh_function_list.pdf`) were exactly the blit-seam class.

The 11 that remain are **not** one cluster, and the count should not be driven
to zero by widening this rule — three of them are real:

| files | cause |
|---|---|
| `bug_554151.{in,pdf}` | tiny-skia fills the page **red**, vello_cpu fills it **white**. A whole-page divergence, 484 704 pixels at 255 counts. A genuine backend or engine defect. |
| `bug_1983.{in,pdf}`, `single_point_paths.{in,pdf}` | one backend paints a degenerate mark (zero-length line, single-point path) and the other paints nothing. The localized form of `one_painted_nothing`. |
| `bug_0_length_line.pdf`, `bug_0_width_line.pdf` | the same degenerate-path family, at ordinary coverage magnitudes. |
| `bug_1288_2.{in,pdf}` | **not an edge phenomenon.** vello_cpu writes a constant 127 where tiny-skia writes 129/130 on a 50% blend — a systematic one-sided rounding bias in compositing, two counts over `INTERIOR_TOLERANCE`. Widening the edge mask would hide it; the fix belongs in whichever pipeline is rounding wrong. |
| `xfermodes3.pdf` | the genuine blit-seam residue: 2x2 cell-corner blocks sitting two pixels in from agreed structure. Radius 2 clears it, at the cost above. |

The prior status note attributed all 13 to the tiling blind spot. Measurement
does not support that: only `xfermodes3` and `bug_1288_2` involve tiling at
all, and `bug_1288_2`'s cause is compositing rounding rather than edges.
