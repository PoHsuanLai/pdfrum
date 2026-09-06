# Issues to file

Every open work item, one per `##` heading, in the shape an issue tracker
wants. Nothing here has been filed. Numbers are quoted from the measurement
that produced them; where a figure was never taken the entry says "not
measured" rather than guessing.

Three sources feed this file: the open rows of the internal work queue, the
standing losses in `docs/benchmarks/losses-explained.md`, and the upstream bug
drafts under `docs/upstream/`.

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

Decode at the size the page actually draws rather than at full resolution: the
image-rows pipeline's reduction stage plus a scaled JPEG decode, the latter
blocked on `zune-jpeg` gaining a `scale_denom` option (see the zune issue
below). An interim nearest-neighbour 1/2, 1/4, 1/8 reduce immediately after
decode, before the general resample, is available without the upstream change.

### Acceptance

Peak resident memory under 1 GiB on that file, with the conformance board
unchanged row for row.

---

## Diagnose the spurious generated space in text extraction

**Labels:** bug, text, conformance
**Area:** `crates/pdfrum-text`

### Context

`crates/pdfrum-text/src/pipeline.rs:696` is `ProcessInsertObject`'s
counterpart, and `crates/pdfrum-text/src/pipeline.rs:1139` is `GenerateSpace`'s.
Both are transcribed line for line from the oracle and the transcription has
been checked against it.

### Problem

`text_quick_start.pdf` extracts at text F1 0.641 against the oracle. The
difference is spurious generated spaces: leading, before a leader run, before a
page number, trailing, plus about fourteen whitespace-only lines. Where the
oracle emits `Different Views......2` we emit
` Different Views ...... 2 `. The same defect costs
`image_ccitt_3bigpreview.pdf` its last 0.001 (F1 0.999 against the character
stream, `Clips Clips` for `Clips` on one line) and part of
`text_tcpdf_055.pdf`'s shortfall.

Two named causes are refuted, not merely untested. The harness's character
stream was fixed and this row did not move: 0.641 before, 0.641 after.
Truncating the glyph width to an integer as `GetCharWidth` does is a measured
no-op — instrumented, the truncation fired zero times across 1420 corpus PDFs,
the 1759-row board is byte-identical, and all 44 benchmark text rows are
unchanged. The dot-leader hypothesis is refuted too: leaders are one line on
both sides.

### Proposed fix

Not determined. What would determine it: `GetCharWidth` and `GetPos`
instrumented per item in a PDFium build, compared against ours on the same
file. The remaining candidates are positional — `GetPos`, the text-matrix
composition, or `FindPreviousTextObject`'s choice of previous run.

### Acceptance

`text_quick_start.pdf` text F1 above 0.641 in the benchmark's text column, with
the four spurious-space sites gone; `image_ccitt_3bigpreview.pdf` at 1.000.

---

## Explain the C0 control-character row in text_tcpdf_055.pdf

**Labels:** bug, text
**Area:** `crates/pdfrum-text`

### Context

`docs/benchmarks/losses-explained.md` records this row at text F1 0.953 as
published and 0.948 against the character stream — it moved down 0.005 when the
harness was fixed, and four peer engines are closer.

### Problem

Two residuals. One is the spurious leading space of the issue above. The other
is a row where we emit the C0 controls `^@ ^A ^B ^C … ^_` and the oracle emits
nothing at all. Those codes are legitimately present in `char_list_` on both
sides, so the oracle's empty row is an input-geometry or charcode-mapping
difference rather than a filtering rule we failed to port.

### Proposed fix

Not determined. The same per-item `GetCharWidth` / `GetPos` instrumentation the
issue above needs would answer this row too; the two are worth doing in one
pass.

### Acceptance

Text F1 back above 0.953 with the C0 row either matched or explained and
recorded as a stated divergence.

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

## Upstream (pdfium): the text-object bbox gate drops standalone spaces

**Labels:** upstream, pdfium, bug
**Area:** `docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md`

### Context

Goes to **pdfium**. Confidence is observed output.

### Problem

The bounding-box gate applied to text objects drops standalone space characters
from extracted text. This is the root cause behind two existing Chromium bugs,
crbug.com/40643656 and crbug.com/444176962, which report the symptom without
naming it.

### Proposed fix

File the draft and link both existing bugs.

### Acceptance

Issue filed and cross-linked.

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
