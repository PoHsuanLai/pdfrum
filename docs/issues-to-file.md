# Issues to file

Every open work item, one per `##` heading, in the shape an issue tracker
wants. Nothing here has been filed. Numbers are quoted from the measurement
that produced them; where a figure was never taken the entry says "not
measured" rather than guessing.

Three sources feed this file: the open rows of the internal work queue, the
standing losses in `docs/benchmarks/losses-explained.md`, and the upstream bug
drafts under `docs/upstream/`.

**To file one.** The format is GitHub-Flavored Markdown with a fixed skeleton,
which pastes unchanged into GitHub, GitLab, Gitea, Jira and Linear: the `##`
line is the issue title, `**Labels:**` and `**Area:**` are metadata, and the
four `###` sections are the body. Copy from the `##` line to the `---` that
ends the entry, drop the two metadata lines into the tracker's own fields, and
paste the rest as the description. For a bulk file:

```sh
gh issue create --title "<the ## line>" --body-file <entry.md> --label "<labels>"
```

Entries titled **Upstream (project)** go to that project's tracker, not ours;
each names its target and the draft under `docs/upstream/` that carries the
full write-up, source citations and repro files.

---

## Close the warm render gap against mupdf

**Labels:** performance, render
**Area:** `crates/pdfrum-render`, `crates/pdfrum-raster-vello-cpu`

### Context

`docs/benchmarks/README.md` publishes run 3, taken on an idle machine at
commit `8a02d57b1fa7`. pdfrum is the slowest median renderer of the five
engines measured.

### Problem

Warm median render is 10.55 ms on the 44-file corpus against pdfium-render
5.68 ms, mupdf 4.18 ms and hayro 8.78 ms; on the 209-file PDFium sample it is
7.37 ms against 3.00, 2.08 and 3.82. Cold render is 41.90 ms and 28.99 ms,
three to nine times the others.

`docs/design/mupdf-comparison.md` §1.1 locates the fixed part of the gap:
`vello_cpu`'s `F32Kernel::pack` and `unpack` are scalar, costing roughly 48
instructions per pixel per render whatever the page contains.

### Proposed fix

§6 of that study ranks three routes: (a) portable changes with no pixel
movement, (b) changes that move pixels but stay inside the conformance floor,
(c) mupdf's structurally different answer, which is not proposed. Route (a) is
the one to take first, and the `vello_cpu` half of it is an upstream request
(see the `vello_cpu` issue below).

### Acceptance

Warm median render, measured on an idle machine through `benches/compare`,
below 8 ms on the 44-file corpus, with the conformance board unchanged row for
row.

---

## Cut peak render memory on image_bug_583804.pdf from 1539 MiB

**Labels:** performance, memory, render
**Area:** `crates/pdfrum-render` image path

### Context

Peak resident memory is the one place pdfrum is strictly behind every peer with
no compensating benefit. `docs/benchmarks/losses-explained.md` records it.

### Problem

Peak render memory on `image_bug_583804.pdf` is 1539 MiB where every other
engine measured stays under 1 GiB — mupdf peaks at 914 MiB. The corpus median
is 51.50 MiB, so this is one document, not a systemic cost. That document
retains 176 MB for a single page with no eviction, and spends 59-75% of its
render inside the image path. Wall time on it is 1.5 s against mupdf's 0.16 s.

### Proposed fix

**The image path was not the cause, and this entry's original diagnosis was
wrong.** Probing `VmHWM` through the rasterize path found the page is 4473 pt
square, so at 150 DPI the target is 9318x9318 and one page-sized RGBA8 buffer
is 331 MiB -- and three existed at once. `VelloCpuDevice` allocated a `base`
pixmap eagerly in `new_target`, read it once to seed the target and dropped
it; `rasterize` then built a second `vello_cpu::Pixmap` and copied it back out
with `to_vec`. That file's own image is drawn *above* 1:1, so there was
nothing for a reduction to reduce.

The fix is in `crates/pdfrum-raster-vello-cpu`: a `Seed::{Solid, Backdrop}`
enum so a uniform clear colour costs four bytes rather than a target-sized
buffer, and lending `vello_cpu` a `PixmapMut` over the result buffer so
neither the second pixmap nor the copy exists.

Decoding at the drawn size remains worth doing for the document shape this
file was mistaken for, and the `zune-jpeg` `scale_denom` request below still
stands -- but it is a separate issue, not this one.

### Acceptance

Peak resident memory under 1 GiB on that file, with the conformance board
unchanged row for row.

### Resolved at `56a01ac` — and the proposed fix was the wrong diagnosis

Measured rather than reasoned about, the cost was not the image path at all.
The page is 4473 pt square, so at 150 DPI the render target is 9318x9318 and a
single page-sized RGBA8 buffer is 331 MiB. Three existed simultaneously: the
`base` pixmap `VelloCpuDevice` allocated eagerly and read once, the
`vello_cpu::Pixmap` `rasterize` rendered into, and the copy `to_vec` made
coming back out. Seeding is now a `Seed::{Solid, Backdrop}` enum and
`vello_cpu` rasterizes through a `PixmapMut` over the result buffer, so one
buffer exists where three did. **1596 -> 916 MiB** on that file and a corpus
render median of **49.73 -> 30.95 MiB**, with zero rows of the 1759-file board
changing their pixel result.

Decoding at the drawn size would have bought nothing here — the image is
4473x4473 drawn into 9318x9318 device pixels, so it is already above 1:1. The
`zune-jpeg` `scale_denom` request below stays worth filing for the shape of
document this file was mistaken for, but it was never this file's problem.

The file also rendered *faster*, not slower: warm 1489 -> 1290 ms, cold
1723 -> 1370 ms. **Close this entry when the change lands.**

---

## Close the closepath seam in stroke expansion

**Labels:** bug, render
**Area:** `crates/pdfrum-render` stroke expansion

### Context

`fx/path/transparent1.pdf` was at SSIM 0.984 and is now at 0.992 after the
knockout fix: the `#930000` pixel count went 12567 to 0 against the oracle's 0,
and `#ff6c6c` went 13200 to 27335 against the oracle's 27586.

### Problem

The remaining 0.41% is a separate defect: at a closed subpath's seam we apply a
join where the oracle applies a cap, or the reverse. It is diagnosed and not
fixed.

### Proposed fix

Not determined beyond the diagnosis. Fixing it means matching the oracle's rule
for what happens at the start and end vertex of a closed stroked subpath.

### Acceptance

SSIM on `fx/path/transparent1.pdf` above 0.992 and toward 1.0, with no board
row moving down.

---

## Measure the cause of example_025.pdf's render SSIM 0.9913

**Labels:** render, investigation
**Area:** `crates/pdfrum-render`

### Context

`third_party/tcpdf/example_025.pdf` renders at SSIM 0.9913 against
pdfium-render's 0.9989 in the benchmark table.

### Problem

The file is above the 0.99 conformance floor and outside the six documents
whose losses have been analysed, so no pixel-difference pass has ever been run
on it. The cause is not measured.

### Proposed fix

Not determined. A pixel-difference pass in the style of
`docs/benchmarks/losses-explained.md`'s method section — signed per-channel
bias, edge correlation, flat-interior agreement — would say whether this is an
antialiasing tradeoff like `shading_tcpdf_058` or a real defect.

### Acceptance

A measured cause recorded in `docs/benchmarks/losses-explained.md`, or SSIM at
or above 0.999.

---

## Halve the stretch and unpack image path

**Labels:** performance, render
**Area:** `crates/pdfrum-render` (`image::to_pixmap`, `stretch::reduce_to`, `unpack`)

### Context

Measured by callgrind instruction counts on
`benches/corpus/text_quick_start.pdf`, all eleven pages at `--scale=2.0833`,
against the oracle's `pdfium_test` on the same input.

### Problem

We spend 3.86 G instructions where the oracle spends 1.96 G for the same
pixels. The oracle's cost is 51% in `CStretchEngine` (0.99 G) and 19% in
`CPDF_DIB::GetScanline` (0.37 G). Ours is 2.15 G in `to_pixmap` plus
`reduce_to` and 0.58 G in `unpack` / `sample_bytes` for that same work, plus
0.58 G in the JPEG decoder.

Three-engine like-for-like through the harness, per pass over eleven pages:
total 7.43 G (pdfrum) against 2.87 G (PDFium) and 3.92 G (mupdf). The image
path alone is 3.6 G for us — `to_pixmap` 1.32 G, `reduce_to` 1.67 G,
`decode_image` 0.63 G, image paint 0.63 G — against PDFium's 2.18 G and mupdf's
2.62 G.

A decoder as fast as libjpeg-turbo would save roughly 0.3 G, about 8%. A scaled
decode saves little on this document because it draws near 1:1. The gap that
matters is our own stretch and unpack, about twice the oracle's for the same
pixels.

### Proposed fix

A pass over `stretch::reduce_to` and `unpack`. One concrete lever named by the
measurement: a fast path in `unpack` for 8-bit one- and three-component images
that skips the bit reader entirely.

### Acceptance

Guide instruction count moving from 3.86 G toward the oracle's 1.96 G, with the
board unchanged row for row.

---

## Take a wall-clock warm-open number on an idle machine

**Labels:** performance, benchmarks
**Area:** `benches/compare`, `crates/pdfrum-parser`

### Context

`crates/pdfrum-parser/src/xref/mod.rs:96` documents the container change that
landed 2026-09-06: `EntryTable` became a vector indexed by object number.
`merge_up` went from 1087558 to 127989 instructions and `add_compressed` from
1398475 to 110450; open on `text_quick_start.pdf` went 5350120 to 2713090
(-49.3%) and on `forms_widgets_407.pdf`, 9950 objects, 14561868 to 4791409
(-67.1%). Zero of 1759 board rows changed.

### Problem

The published warm-open median is still 0.14 ms against hayro's 0.04 ms and
pdf-rs's 0.12 ms, taken before that change. It has not been re-measured,
because the machine ran at load 20-40 of 32 cores and could not resolve the
difference. The largest remaining item inside open is `catalog_page_count` at
23.6%, which is the eager page-tree walk and stays deliberately: it is what
makes `Document::page_count()` infallible, and it is why we open 208 of 209
sample files where pdf-rs fails on 72 and lopdf and pdf-extract on 46.

### Proposed fix

Re-run `benches/compare --label open` on an idle machine and republish the
column.

### Acceptance

A warm-open median measured at machine load below 2, published in
`docs/benchmarks/README.md` beside hayro's 0.04 ms.

---

## Re-measure the ratchet's noise bands now the heavy documents are ordered last

**Labels:** benchmarks, harness
**Area:** `benches/src/bin/ratchet.rs`, `crates/pdfrum-render/benches/render.rs`

### Context

Attributing the 2026-09-06 ratchet run's 34 regressions found that 30 were not
regressions. The cause was measured: all 44 documents in a render group share
one process, and the three pathological image documents left a resident set the
rows after them paid for — `image_bug_583804` alone took the bench binary's
peak RSS from 27,184 kB to 520,980 kB.

The harness now orders those three last within each group, extending a
distinction `cold_samples` and `warm_samples` already drew for the same three
stems. That removes the one perturbation large enough to clear a band.

### Problem

The bands in `default_bands()` — 3% to 5% on the render groups — were measured
before that change, on a harness whose reproducibility was worse than the bands
themselves. Two runs of the same binary at the same commit reported 34 and 27
regressions sharing only 13 rows, with the warm cluster moving bodily between
backends. So the bands describe a noise floor that no longer applies, and it is
not known whether they are now too tight, about right, or loose enough to hide
a real regression.

### Proposed fix

Re-measure them the way `docs/status/M12.md` §2 originally did: several
consecutive runs at one commit on the idle reference box, and take each group's
band from the observed run-to-run spread. The test of a band is that two
consecutive runs at the same commit agree row for row within it.

### Acceptance

Bands re-derived from post-change runs and written into `benches/baseline.json`,
with two consecutive runs at one commit reporting zero regressions.

---

## Stop extract-font-tables.py regenerating the removed rustdoc

**Labels:** chore, tooling
**Area:** `scripts/extract-font-tables.py`, `crates/pdfrum-font/src/encoding/tables.rs`

### Context

`crates/pdfrum-font/src/encoding/tables.rs` is marked `@generated`. The rustdoc
pass rewrote its fifteen table docs from a bare `` `kFoo` (path.cpp). `` to a
sentence naming the code page each table encodes.

### Problem

The generator still emits the old form: two `chunks.append` templates around
lines 147 and 160 of `scripts/extract-font-tables.py`. Re-running it reverts
all fifteen docs. Nothing in `scripts/ci.nu` regenerates the file, so this is a
trap for the next person rather than a live break. Separately, the generator's
`HEADER` names a `scripts/extract_tables.py` that does not exist.

### Proposed fix

Give the two templates the same treatment the checked-in file got, and correct
the `HEADER` path.

### Acceptance

Re-running the extractor leaves `crates/pdfrum-font/src/encoding/tables.rs`
byte-identical.

---

## Wire Doc.calculateNow through the form pipeline

**Labels:** feature, forms, javascript
**Area:** `crates/pdfrum-form`

### Context

`crates/pdfrum-form/src/script/doc.rs:578` implements `Doc.calculateNow()` as a
request flag rather than a call, because running the sweep from inside a native
function would re-enter the cascade the script is already inside — which the
oracle refuses too. `crates/pdfrum-form/src/script/host.rs:62` holds the flag
and `crates/pdfrum-form/src/script/mod.rs:665` drains it.

### Problem

Nothing reads the drained flag. It is a missed wire, not dead code: the value
is computed correctly and discarded.

### Proposed fix

Three changes in the form pipeline: read the request back after a script run
returns, run `Cascade::calculate` then, and regenerate the appearances the
sweep touches.

### Acceptance

Not determined — no conformance fixture exercises it today, and the two
comparable missing wires moved the board by zero rows. A unit test asserting
that a script calling `calculateNow()` triggers exactly one sweep is the
minimum.

---

## Decode JPEG at reduced resolution once zune-jpeg supports it

**Labels:** performance, blocked-upstream
**Area:** `crates/pdfrum-page/src/image/dct.rs`

### Context

`crates/pdfrum-page/src/image/dct.rs:97` ports the oracle's reduced-resolution
rules — `scale_denominator`, `scaled_size`, `allows_reduced_resolution` — and
all three carry an `allow(dead_code)` because no decode path can ask for them.

### Problem

`zune-jpeg`'s `DecoderOptions` has no `scale_denom`, re-checked 2026-09-03
against 0.5.16-rc1. Without it the decoder always produces full-resolution
samples, which is the direct cause of the peak-memory issue above on documents
that draw a large image small.

### Proposed fix

Ask upstream (draft in `docs/upstream/zune/scaled-decode.md`). In the meantime,
a nearest-neighbour 1/2, 1/4, 1/8 reduce immediately after decode and before
the general resample gets most of the memory saving without the upstream
change.

### Acceptance

The three `dead_code` allowances in `dct.rs` removed because a caller uses
them, and the peak-memory issue's acceptance met.

---

## Census the corpus for deep clip stacks and layer pushes

**Labels:** benchmarks, investigation
**Area:** `benches/corpus`, `crates/pdfrum-raster-agg`

### Context

Two landed optimisations were measured on the fixtures that motivated them and
nowhere else: the AGG clip-push change, and the layer-composite change that
took a layer arm from 11.47 ms to 2.08 ms and a whole render from 17.03 ms to
9.32 ms, a 1.83x.

### Problem

Neither has a known reach. No document outside the forms fixture was looked for
that has a deep clip stack, and no census was taken of which corpus documents
push layers at all — all five controls used push none. A document that would
gain from either change has not been searched for.

### Proposed fix

Instrument a corpus pass that records maximum clip-stack depth and layer-push
count per document, and re-measure any fixture the census turns up.

### Acceptance

The census published, and any newly found fixture measured before and after
both changes.

---

## Diagnose the six corpus rows still above 1.5x like-for-like

**Labels:** performance, render
**Area:** `crates/pdfrum-render`, `crates/pdfrum-raster-agg`

### Context

The like-for-like ratio against the oracle is now 0.36x across all rows, down
from 0.52x for the vector class and 0.78x for forms. Six rows remain above
1.5x, down from nine.

### Problem

The six are three image decode/resample rows, `shading_type4_5` in the
sub-millisecond regime where the oracle's own column is under 1.1 ms and the
ratio is least trustworthy, and two residues of the layer and clip work. No
single large line remains in any of them: `shading_tcpdf_058` at 2.29x, once
the largest ratio in the corpus, now spreads its residue across `draw_image`,
`push_layer` and the sweep, with the layer arm at 2.08 ms and `coverage_of` at
1.71 ms.

A census found none of these rows has off-page geometry — 0 of 730, 0 of 144,
0 of 518, 0 of 418, 0 of 52 paths off the device — against
`shading_tcpdf_058`'s 17 of 81, so the earlier explanation does not apply.

### Proposed fix

Not determined. Each row needs its own callgrind diagnosis; there is no shared
cause left.

### Acceptance

All rows under 1.5x like-for-like against the oracle.

---

## Decide whether to expose a per-backend U8Kernel render mode

**Labels:** performance, decision, render
**Area:** `crates/pdfrum-raster-vello-cpu`

### Context

`vello_cpu`'s `RenderMode::OptimizeSpeed`, together with its `u8_pipeline`
cargo feature — without the feature the mode is a silent no-op, since the
pipeline is chosen by `#[cfg]` — was measured on 2026-09-06 and the change
reverted.

### Problem

It is deterministic: two full 1759-row boards came out identical. It is worth
1.9x to 3.4x marginal instruction count and 1.51x warm wall clock, which puts
us at 0.75x to 1.19x of mupdf where the f32 pipeline is 1.24x to 3.46x. But it
moves fourteen rows from pass to fail — every one a
`corpus/fx/FRC_8.2.4_part1/FRC_*` file, all sitting at SSIM 0.990010 and
dropping to 0.989976, one part in 10^5 above the 0.99 floor. Median SSIM moves
by one part in 10^6 and the byte-exact count is unmoved at 499.

That makes it wrong as a default and possibly right as an option.

### Proposed fix

A type-driven per-backend render-mode knob defaulting to `OptimizeQuality`, so
a caller who wants the speed can ask for it and accept the fourteen rows. Not a
flipped default.

### Acceptance

The knob exists and is documented with the fourteen-row cost; the default board
stays at its current pass count.

---

## Implement public-key (Adobe.PubSec) encryption

**Labels:** feature, crypt
**Area:** `crates/pdfrum-crypt`

### Context

One file of the 209-file PDFium sample fails to open:
`fx/FRC_3.5_part1/…SubFilter_s5.pdf`. `pdf_oxide` opens it; mupdf and
pdfium-render also decline it.

### Problem

The standard security handler is implemented; `Adobe.PubSec` is not, so a
document whose `/Encrypt` names a recipient list rather than a password errors
at open.

### Proposed fix

A PKCS#7 recipient-list decryptor and a certificate store to resolve against.

### Acceptance

209 of 209 files opening in the benchmark's open column.

---

## Re-record the conformance scoreboard for util_printd

**Labels:** chore, conformance
**Area:** `conformance/scoreboard.json`

### Context

The `util.printd` timezone fix landed 2026-09-06 and took the `js-transcript`
failure count from 8 to 7, turning `util_printd.in#js-transcript` from fail to
pass.

### Problem

The committed `conformance/scoreboard.json` still records the old `Sunday`
mismatch for that row.

### Proposed fix

Re-record the board.

### Acceptance

`conformance/scoreboard.json` carries no `Sunday` mismatch for `util_printd`,
and the recorded pass count matches a fresh run.

---

## A decimal number opening a sentence is read as a list marker

**Labels:** bug, markdown
**Area:** `crates/pdfrum-markdown/src/heuristics.rs`

### Context

`leading_number` recognises a multi-part label like `2.1 Background` by its
shape: digit groups joined by points, closed by whitespace
(`heuristics.rs:514-517`, the `groups > 1` arm). That rule is what makes a
numbered table of contents come out as a list.

### Problem

The same shape is a decimal number, so a sentence opening with one becomes a
list item. Measured over `benches/corpus`, three real cases:

- `1.3 GHz or faster processor.` -- a hardware requirement, rendered `- 1.3 GHz
  or faster processor.`
- `0.1 Contents` -- a table cell on `vector_en_tem.pdf`'s revision history
- `14.2 朗读设置：...` -- correct here, a genuine section number

So the rule is right more often than it is wrong on this corpus, which is why
it has stood. It is still an over-match, and it was found by an over-match
probe that failed: `leading_number("3.14 is pi")` returns `Some`.

### Proposed fix

Not determined, and worth thinking about before coding. Candidates, none
verified:

- A list marker is followed by a capital or an ideograph, not by a lower-case
  word: separates `1.3 GHz` (upper) poorly, but `3.14 is pi` (lower) well.
- A marker's number continues its neighbours' sequence. `2.1` after `2.0` is a
  marker; a lone `1.3` among prose lines is not. This is the strongest signal
  and needs the list machinery to look at runs rather than single lines.
- A decimal has at most one point and no trailing separator; a label often
  carries a closer. Weak on its own.

Whatever is chosen must keep `2.1 Background ... 4` and `14.2 朗读设置` working,
both of which are genuine markers in the corpus today.

### Acceptance

`leading_number("3.14 is pi")` is `None` and `1.3 GHz or faster processor.`
renders as a paragraph, with `text_quick_start.pdf`, `vector_en_tem.pdf` and
`text_cjk_functions.pdf` unchanged in their genuine lists, and a unit test
pinning each case.

---

## Upstream (vello): SIMD-ify F32Kernel pack and unpack

**Labels:** upstream, vello, performance
**Area:** `docs/upstream/vello/pack-unpack-simd.md`

### Context

Goes to **vello** (`vello_cpu`), github.com/linebender/vello. Draft written
2026-09-06, not filed.

### Problem

`F32Kernel::pack` and `unpack` are scalar, costing roughly 48 instructions per
pixel per render — a fixed cost per rendered page independent of page content.
It is the largest single component of pdfrum's 2.5x warm gap against mupdf.

Checked for duplication: both `// TODO: SIMDify` comments are still on upstream
`main` at the released line numbers, no open or closed issue or pull request
matches, and 0.2.0 is the latest release with an empty `[Unreleased]` changelog
for fine-kernel work.

### Proposed fix

The measurement and the suggested shape are in the draft; the supporting
analysis is `docs/design/mupdf-comparison.md` §1.1 and §8.

### Acceptance

Issue filed on linebender/vello with the per-pixel instruction measurement.

---

## Upstream (hayro): JBIG2 segment bodies read past the available length

**Labels:** upstream, hayro, bug
**Area:** `docs/upstream/hayro/jbig2-segment-lengths.md`

### Context

Goes to **hayro** (`hayro-jbig2`). 0.3.0 is still the latest release and no
upstream issue covers it.

### Problem

On a truncated JBIG2 stream, segment bodies are read at their declared length
rather than at the length actually available.

### Proposed fix

File the draft as written.

### Acceptance

Issue filed with the repro.

---

## Upstream (zune): decode JPEG at 1/2, 1/4 or 1/8 inside the IDCT

**Labels:** upstream, zune, feature
**Area:** `docs/upstream/zune/scaled-decode.md`

### Context

Goes to **zune** (`zune-jpeg`). This one is already asked for: zune-image issue
434, opened 2026-08-18 by someone else and still open.

### Problem

`DecoderOptions` has no `scale_denom`, so a caller that needs a quarter-size
image must decode full-size and then resample — the direct cause of pdfrum's
peak-memory outlier. PDFium itself moved to zune-jpeg on 2026-09-03 and
box-averages after a full decode for the same reason.

### Proposed fix

Post our measurements as a comment on that issue rather than filing a
duplicate.

### Acceptance

Comment posted with the memory and instruction-count figures.

---

## Upstream (pdfium): a /TR function with over 16 outputs renders black

**Labels:** upstream, pdfium, bug
**Area:** `docs/upstream/pdfium/tr-single-function-unwritten-output.md`

### Context

Goes to **pdfium**. The strongest of the five unfiled PDFium drafts: confidence
is observed output, not code reading. Repro files were added 2026-09-05.

### Problem

A single-function `/TR` transfer function with more than sixteen outputs leaves
an output unwritten, and the page renders entirely black.
`docs/upstream/repro/tr_single_function_17_outputs.pdf` renders black where its
two controls render grey. The two neighbouring `/TR` fixes of 2026-09-03 left
this branch as it was.

### Proposed fix

File the draft with the three repro PDFs.

### Acceptance

Issue filed on the PDFium tracker.

---

## Upstream (pdfium): the empty-box text gate drops real letters, not only spaces

**Labels:** upstream, pdfium, bug, text-extraction
**Area:** `docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md`

### Context

Goes to **pdfium**. Confidence is observed output, measured against
`pdfium_test --txt` on the checkout's own fixtures.

`CPDF_TextPage` gates each text object on the width of its glyph bounding box
(`core/fpdftext/cpdf_textpage.cpp:881` and `:1076`, against `kSizeEpsilon` at
`:42`). A glyph whose outline encloses no area -- a space, and some fonts'
letters -- has an empty box while its `w0` displacement per ISO 32000-1
Sec. 9.4.3 is not zero, so the object is discarded before extraction.

### Problem

The known symptom is lost spaces, reported twice upstream without the cause
being named (crbug.com/40643656, crbug.com/444176962). **The stronger finding
is that the same gate drops running prose.** On
`testing/resources/bug_921.pdf`, `pdfium_test --txt` begins mid-sentence at
"разве не выражает" where the page draws "И разве не выражает": five
characters of Russian -- an `И`, an em dash, a `в`, a `я` and a second `И` --
are absent from the output. Their glyph boxes are empty and their `w0` is
5.3-11.3.

This is a data-loss defect rather than a spacing one, and the letters case is
the better repro because it cannot be mistaken for a whitespace-normalisation
question.

### Proposed fix

Gate on the object's displacement rather than its glyph box, with two
exclusions that a naive advance test gets wrong -- both found by measurement,
not by reading:

- **The scalar must not be Unicode whitespace**, not merely `U+0020`. Fifty
  `annots/` fixtures draw `U+00A0` in an empty-box object; a `U+0020`-only
  test appends a trailing space to every one.
- **The scalar must not be a C0 or C1 control.** `text_tcpdf_055.pdf` and
  `bug_651304.pdf` draw control codes in empty-box objects and the gate is
  right to drop those.

Where the object carries no `ToUnicode` mapping, read the character code, as
`cpdf_textpage.cpp:1213-1215` already does elsewhere. That is the property
that separates the letters worth keeping from the controls worth dropping:
both families have an empty mapping, and only the code distinguishes them.

### Acceptance

Issue filed against pdfium with both existing Chromium bugs cross-linked, and
the `bug_921.pdf` letter loss leading rather than the whitespace symptom. A
downstream implementation of the predicate above is measured over a 1759-file
corpus: it recovers the five characters, keeps every control-code case the
gate currently gets right, and moves 124 files from failing to passing on
their text artifact with none moving down.

---

## Upstream (pdfium): shading ramp skew and radial truncation

**Labels:** upstream, pdfium, bug
**Area:** `docs/upstream/pdfium/shading-ramp-and-radial-truncation.md`

### Context

Goes to **pdfium**. Confidence is code-level with a contrived trigger.

### Problem

The shading colour ramp is skewed and the radial shading is truncated. Real,
but low salience: the error is one part in 256.

### Proposed fix

File the draft, saying plainly that the trigger is contrived.

### Acceptance

Issue filed.

---

## Upstream (pdfium): IsPunctuation's range test misclassifies a character

**Labels:** upstream, pdfium, bug
**Area:** `docs/upstream/pdfium/ispunctuation-range-typo.md`

### Context

Goes to **pdfium**. Confidence is code-level; the scope is arguable.

### Problem

`IsPunctuation`'s range test uses `<=` where it should not, so one character is
classified as punctuation that is not. The `<=` is plainly a typo; the
cp1252-versus-Unicode question underneath it is a design question, not a bug.

### Proposed fix

File the typo half of the draft; raise the cp1252 half as a question rather
than a claim.

### Acceptance

Issue filed.

---

## Upstream (pdfium): EnableStdConversion never reaches a pixel

**Labels:** upstream, pdfium
**Area:** `docs/upstream/pdfium/std-conversion-never-reaches-a-pixel.md`

### Context

Goes to **pdfium**. Confidence is code-level plus a null result over 1757
renders.

### Problem

`EnableStdConversion` is a dead mechanism: no configuration of it changes any
rendered pixel. This is a dead mechanism rather than a defect, and it is the
lowest priority of the five.

### Proposed fix

File as a cleanup suggestion, or drop it. The measurement is the value here.

### Acceptance

Filed, or explicitly dropped.

---

## Markdown: tables come out as prose

**Labels:** markdown, heuristics
**Area:** `crates/pdfrum-markdown/src/heuristics.rs`

### Context

`pdfrum extract markdown benches/corpus/vector_en_tem.pdf` flattens the
revision-history table to two prose lines:

```
Version Author Date Description

- 0.1 Contents
```

`Block::Table` exists in the AST and `render.rs` writes it as a GFM pipe
table; nothing ever constructs one on the heuristic path. Only the tagged
path (`tagged.rs`, `"TABLE"`) does, and none of the corpus fixtures checked
here is tagged.

### Problem

There is no table detection from geometry. The work splits three ways, and
only the first two are reachable without a model.

**Tier 1 — wired (ruled) tables from vector geometry.** `~/MinerU-rs/crates/
mineru-table/` needs a UNet only because it works from a rasterized page and
must infer where the ruling lines are from pixels. We do not have that
problem: `pdfrum-page` exposes ruling lines as literal path objects with
exact coordinates, so we can produce a *better* line set than the model by
reading the content stream, then feed MinerU's classical recovery unchanged.
That is replacing an estimate with ground truth, not lossy distillation.

Everything needed is already public and needs no change to `pdfrum-page` and
no new dependency (`kurbo` is already a dependency of this crate):

- `PageObject::Path(Box<Content<PathObject>>)` — `page.rs:254`.
- `PathObject { path: BezPath, matrix: Affine, fill_rule: FillRule, stroke:
  bool }` — `page.rs:96`. Page-space geometry is `matrix * path`; the
  bounding box is `matrix.transform_rect_bbox(path.bounding_box())`, the
  idiom `lines.rs:164` already uses for images.
- `Content::state.stroke_params.width` for thickness and
  `state.fill`/`state.stroke` to drop white or faint rules.
- `lines.rs:109` `ObjectFacts::from_page` already walks every object and
  discards `PageObject::Path(_)`; a parallel `paths` field slots in beside
  `images`. It recurses into forms **without** accumulating
  `FormObject.matrix`, so a parent `Affine` must be threaded through
  `collect` for rules drawn inside a form.

What to port, cited file by file, from `~/MinerU-rs/crates/mineru-table/`:
`unet/recover.rs` (383 lines, logical grid inference from cell rectangles)
and `unet/postprocess.rs` (309 lines, assembly with rowspan/colspan).
`unet/extract.rs`'s morphology and connected-component labelling is *not*
needed — it exists to turn a predicted mask into rectangles, and we have the
rectangles. `matching.rs` (OCR-to-cell matching) has an analogue worth
reading: ours matches extracted `Line`s to cells, which is easier because we
have exact text positions. Do **not** port `unet/model.rs` or anything under
`generated/` — those are the network.

**Tier 2 — borderless tables by column alignment.** Text at consistent
x-positions across several rows, the classical pdfplumber/camelot approach.
Covers most business documents; fails on merged cells and ragged rows.

**Declined — irregular borderless tables.** MinerU answers these with
SLANet, a CNN plus attention decoder emitting HTML structure tokens end to
end. There is no mask to substitute with ground truth, so nothing about it
is portable. This crate takes no model weights and no ML dependency, so
irregular borderless tables are out of scope, permanently, by design.

### Why this was filed rather than built in the markdown-quality pass

The named fixture would not have been fixed by either buildable tier.
`vector_en_tem.pdf`'s revision-history table has no ruling lines (tier 1
does not apply) and its one data row is ragged — four headers, `Version
Author Date Description`, against a two-cell row `0.1 Contents`, as
`extract text --layout` shows. Column alignment cannot recover a row whose
cells do not line up under the headers, so tier 2 does not apply either.
It is precisely the irregular borderless case that is declined above.

Building tier 1 is worthwhile on its own merits — ruled tables are common —
but it is a project rather than part of a pass, and it would have shown no
improvement on any fixture that pass was measured against.

### Proposed fix

Tier 1 as its own change, on a fixture with a ruled table. Then tier 2,
scoped separately.

### Acceptance

A ruled table in a corpus fixture comes out as a GFM pipe table with the
right number of rows and columns, and no currently-clean fixture regresses.
