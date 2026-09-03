# The conformance harness

Six subcommands over the oracle checkout and the golden store:

```
conformance generate-goldens   # run pdfium_test, record its answers
conformance run                # Tier A (byte-exact dumps) + Tier B (pixels)
conformance run --check-regressions conformance/scoreboard.json
conformance triage             # cluster the scoreboard's failures
conformance tier-c             # the two rasterizers against each other
conformance save-round-trip    # we save, the oracle reopens (M7/M10)
conformance mutate-round-trip  # we edit and save, both render it (M11)
```

The last two are the only ones that need **both** binaries, because
`pdfium_test` cannot write a document at all: the check is a sequence rather
than a flag diff.

`pdfrum-tool` has its **own** `script` feature. Build it with
`cargo build -p pdfrum-tool --release --features script`; passing
`--features pdfrum/script` instead compiles cleanly and silently loses nine
`js-transcript` rows (42 failing instead of 33), because the tool's transcript
path is behind the tool's feature, not the facade's.

`--tool` names the `pdfrum-tool` binary and defaults to
`target/release/pdfrum-tool` **under the repository root**, which is not where
a workspace with a redirected `CARGO_TARGET_DIR` puts it. When `tier-c`
reports `0 files compared`, that is the reason; pass `--tool` explicitly.

`--goldens` has the same shape of trap. `conformance/goldens/` is
`.gitignore`d, so a fresh worktree has none, and a run without the flag reads
every row as `missing-golden` **and still exits 0** — `--check-regressions`
cannot regress against nothing. From a worktree, name the main checkout's
store: `PDFRUM_GOLDENS=/path/to/pdfrum/conformance/goldens`.
`--checkout` resolves the same way and needs the same treatment
(`PDFRUM_ORACLE_CHECKOUT`); its parent directory is the same silent zero.
A board that
reports no passes and no failures has not run — and it has still overwritten
`conformance/scoreboard.json` on the way out, so restore that from git before
the next `--check-regressions` — or write a trial run to `--out` under a
scratch path in the first place. `--out` is a **file** path, not a directory:
pointing it at one fails with `Is a directory` and still exits 0, the same
silence as the rest of this paragraph. The same covers a missing `--tool`:
every row reads `unsupported-tool`, and the exit code is still 0.

## The four paths, and the variables that name them

Every path outside this repository comes from one place — an environment
variable with a default relative to the repository root — so a fresh clone
resolves all four without an edit and a differently laid out machine overrides
only what differs. The flags below still win over the variables.

| variable | flag | default |
|---|---|---|
| `PDFRUM_ORACLE_CHECKOUT` | `--checkout` | `<repo>/../pdfium-c++` |
| `PDFRUM_ORACLE_BIN` | `--oracle` | `<checkout>/out/Release/pdfium_test` |
| `PDFRUM_GOLDENS` | `--goldens` | `<repo>/conformance/goldens` |
| `PDFRUM_TOOL` | `--tool` | `<repo>/target/release/pdfrum-tool` |

`scripts/env.nu` resolves the same variables with the same defaults for the
nushell scripts, and the integration tests that need the oracle read the same
two `PDFRUM_ORACLE_*` variables and **skip with a printed message** when the
binary they name is absent.

A board run from a worktree therefore needs no flags at all:

```bash
PDFRUM_ORACLE_CHECKOUT=/path/to/pdfium-c++ \
PDFRUM_GOLDENS=/path/to/pdfrum/conformance/goldens \
PDFRUM_TOOL="$CARGO_TARGET_DIR/release/pdfrum-tool" \
  cargo run -p conformance --release -- run
```

`--limit N` truncates the corpus listing to its first `N` entries. It is a
smoke-test switch, not a filter: there is no way to select a named file.

## What `mutate-round-trip` compares, and why it is not a golden

Every other pixel check here diffs our render against a golden the oracle made
from the original file. That cannot answer the question page mutation raises,
because the file under test is one neither implementation has ever seen: we
edit a page, regenerate its content stream, and write a new document. So the
comparison is between two renders of *that* file — the oracle's and ours. A
regenerated stream only our own interpreter reads back correctly fails here and
passes everywhere else.

`--mutate=` offers three edits, chosen to break in different places:
`add-rect` (a brand-new streamless object and the `/Contents` shape transition
that holds it), `remove-first` (removal bookkeeping and index collapse), and
`touch-all` (the emitter itself, under an edit that changes nothing).

Two things the sweep had to learn, both of which cost a wrong diagnosis:

**SSIM cannot tell a mutation's loss from a disagreement that predates it.**
The metric is local, so painting a large flat rectangle turns busy 8x8 windows
into flat ones — and a file whose handful of wrong pixels scored 1.000
unmutated can score 0.986 mutated without a single new pixel having gone
wrong. The baseline pass therefore counts *pixels* rather than comparing two
SSIM numbers, which is a question with an exact answer: did the two
implementations disagree at all? Both numbers are reported either way.

**The baseline is a plain save, not the original file.** Saving changes some
files even with nothing edited — the writer corrects a `/Length` that lied, so
a stream the original hid behind `/Length 0` comes back with its real payload
and the oracle then draws it. That is the writer's behaviour, identical under
`--save` and `--mutate=`, and charging it to a mutation sends somebody hunting
through the regenerator for a bug that is not there.

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

The 11 that remained were **not** one cluster, and the count was never going
to be driven to zero by widening this rule — most of them were real. Five of
the six named classes were engine bugs, and M8 fixed them:

| files | cause | outcome |
|---|---|---|
| `bug_554151.{in,pdf}` | tiny-skia fills the page **red**, vello_cpu **white**: 484 704 pixels at 255 counts, immune to dilation because there is no agreed boundary anywhere on the page. | **Fixed.** `/Decode` was being applied to samples the oracle never decodes — a row past the end of the stream skips the decode entirely. Byte-exact. |
| `bug_1983.{in,pdf}`, `single_point_paths.{in,pdf}`, `bug_0_length_line.pdf`, `bug_0_width_line.pdf` | one backend paints a degenerate mark and the other paints nothing. | **Fixed.** `BuildAggPath`'s one-pixel nudge for a degenerate subpath, so both backends receive a real segment to stroke. |
| `bug_1288_2.{in,pdf}` | **not an edge phenomenon.** A systematic one-sided rounding bias in compositing, two counts over `INTERIOR_TOLERANCE`. | **Reduced, not cleared.** The bias was not in `AlphaMerge` — that was already exact. An uncoloured tile composites through `CompositeMask`, carrying one flat colour and merging only alpha; we recoloured into premultiplied RGBA and blitted, blending the colour against itself once per overlap. Fixing the shape halved the outliers. The residual is a rasterizer's rounding in the final `draw_image`, three counts at most. |
| `xfermodes3.pdf` | the genuine blit-seam residue: 2x2 cell-corner blocks two pixels in from agreed structure. Radius 2 clears it, at the cost above. | Unchanged, and still a metric artefact rather than a defect. |

Hard failures now: **5**, over two distinct causes. That the rule *found*
five real engine bugs and only ever mislabelled one file is the argument for
leaving the interior guarantee exactly where it is.

The prior status note attributed all 13 to the tiling blind spot. Measurement
did not support that, and the correction was worth more than the two files it
cleared: `bug_554151`'s whole-page divergence was the single most valuable
thing the tier-C run was saying, and it sat unread under a heading calling all
thirteen a blind spot.
