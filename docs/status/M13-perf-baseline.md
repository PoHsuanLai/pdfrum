# M13 — The oracle side-by-side M12b and M12d owed, and what it found

**Taken 2026-09-02.** This is the measurement `docs/status/M12b.md` §10 item 2
names as owed and `docs/status/M12d.md` records as still unpaid at close:
*"`forms` warm stands at M12's 3.35x, unmeasured since… **Any claim about
pdfrum's standing against the oracle today is a claim about M12's measurement,
not this one's.**"* It re-measures the four render classes M12 §1.10 published
warm ratios for, against the same oracle binary and the same 44-file corpus.

**Read §1 and §4 if you read nothing else.** §1 is the answer to the question
that was asked. §4 is the answer to a question nobody asked, which is larger
than the one that was.

---

## 1. The result, stated first

**All four figures moved, all four moved the wrong way, and one commit accounts
for essentially all of it.**

| class | M12 §1.10 warm | **re-measured** | move | verdict |
|---|---:|---:|---:|---|
| `image` | 0.24x | **0.53x** | 2.26x worse | **moved-down** |
| `vector` | 0.90x | **1.98x** | 2.20x worse | **moved-down** |
| `shading` | 0.95x | **2.95x** | 3.11x worse | **moved-down** |
| `forms` | 3.35x | **7.32x** | 2.18x worse | **moved-down** |

Not one is confirmed. The whole-corpus geomean over these four classes goes from
0.83x to 2.02x.

**The cause is a single line in `266783f`** ("Add the second face a field needs
when its own font cannot write a word", M14, 2026-09-01), found by `git bisect`
on `shading_coons` and confirmed by direct excision on four documents in three
classes. It made the annotation-appearance overlay unconditionally load a
synthesized substitute font **on every render of every document carrying any
annotation** — which is most of the corpus, not only the `forms` class.

**The engine itself has not regressed.** Measured with the overlay off, the page
content halves of the affected documents sit where M12 left them:
`shading_coons` 0.68 ms against a 1.04 ms committed baseline,
`image_bug_718762` **0.035 ms** against 0.484 ms. Everything that moved, moved in
the overlay.

---

## 2. What was measured, and on which loop

**The loop is `render-warm`, and the reason is that the oracle is warm.**
`pdfium_test --render-repeats=n` loops `ProcessPage(i)` n times inside one
process (`testing/pdfium_test/pdfium_test.cc:1804`), re-loading and re-rendering
each page while `CPDF_PageImageCache` and the font database stay warm across all
n. The comparable pdfrum loop holds one `RenderSession` across iterations and
rebuilds the page graph inside them, which is exactly
`crates/pdfrum-render/benches/render.rs`'s `warm` group and exactly what M12
§1.8 established the target is judged on.

**It is deliberately not `--sample`.** `scripts/profile.nu`'s header warns that
the plain render loop rebuilds the page graph every iteration while `--sample`
hoists it, and that an A/B run on the wrong loop produced a reproducible +5%
regression that did not exist (`M12b-P3.md` §4). That warning is about A/Bing an
*engine change*, where the rebuild is noise around the thing under test. It does
not apply here and following it would have been wrong: the oracle rebuilds too,
so hoisting our graph and not theirs would measure our best case against their
ordinary one — the same error in the opposite direction from the one M12 §1.8
corrected. `--sample` hoists the *graph* and answers "which half of our engine";
`--warm` hoists the *caches* and answers "how fast are we against them".

**The method reproduces M12 exactly before it is trusted with anything new.**
Taking `benches/baseline.json`'s committed `render-warm-{agg,tinyskia,vello-cpu}`
medians, best-of-three-backends per file, over M12 §1.6's oracle column,
reproduces all four published geomeans and all four class totals to the digit:

| class | recomputed | M12 §1.10 published | class total |
|---|---:|---:|---:|
| `image` | 0.24x | 0.24x | 744.9 ms = 744.9 ms |
| `vector` | 0.90x | 0.90x | 264.6 ms = 264.6 ms |
| `shading` | 0.95x | 0.95x | 87.1 ms = 87.1 ms |
| `forms` | 3.35x | 3.35x | 140.3 ms = 140.3 ms |

So the M12 column below is M12's own arithmetic, not a re-derivation of it, and
a move in the table is a move in the engine or in the machine — not in the
method.

**Fixtures and backends.** All 30 files of the four classes are present under
their M12 names; nothing was renamed or substituted. The *backends* were: the
crates are now `pdfrum-raster-agg` (was `-exact`) and `-vello-cpu` (was
`-vello`), and the criterion groups are `render-warm-agg` / `-vello-cpu`. The
`profile` binary accepts both spellings, so M12-era commands still run. Each
file is measured on all three and the **best** is taken, as M12 did.

**Oracle.** `scripts/bench-oracle.nu` at `n=21`, best of 5, against
`/mnt/data2/pdfium/pdfium-c++/out/Release/pdfium_test` — the same binary and the
same invocation M12 §1.6 used. The script was fit for purpose and is unchanged.
PDFs were copied to a scratch directory first, because `pdfium_test` writes
beside its input and the oracle tree is read-only.

---

## 3. The machine, and what it costs this table

**Every absolute number here is inflated, and the inflation is not uniform
between the two sides.** The box was running at a load average of **47 rising to
65 on 32 cores** throughout — two unrelated `python3` jobs at 450–490% CPU and a
dozen `osmium` processes belonging to another user. These are the same
conditions `M12d.md` records at its own close.

Measured rather than asserted: on the oracle side, `bench-oracle.nu`'s 30 files
come out at a **1.18x geomean** over M12 §1.6's column, on a binary that has not
changed. On our side, an unchanged document measures **~1.5–2x** its committed
baseline (`vector_paths_1751` warm-agg: 21.24 ms against 10.44 ms).

That asymmetry — 1.18x on their side, 1.5–2x on ours — is real and is not
explained by threading (`vello_cpu` runs at `num_threads: 0`). It means **§1's
ratios are upper bounds**, and the true post-fix figures will be better than
they read. It does **not** rescue any of the four: the fix in §4 is measured
against a control on the same machine in the same minutes, so the diagnosis is
load-independent even where the absolute ratios are not.

**This table should be re-taken on an idle box before any of its absolute
numbers are quoted.** What should not wait is §4.

---

## 4. The finding: one commit, four classes, most of the corpus

### 4.1 How it was found

The `forms` re-measurement came back at 7.32x against 3.35x, and the other three
classes came back 2–3x worse as well — which is not a shape a forms regression
has. `shading_coons` was picked as the sharpest instance (1.04 ms committed,
8.00 ms measured) and bisected with criterion itself as the oracle,
`cargo bench -p pdfrum-render -- render-warm-agg/shading/shading_coons`, over the
242 commits between M12's close (`e9d63bd`) and the AGG rename (`947009c`).

The readings were a clean step, not a drift — every good commit 1.38–1.75 ms,
every bad one 6.76–11.84 ms — and `git bisect run` named **`266783f`**.

### 4.2 The mechanism

`266783f` added this to `ap::FormFonts::load`
(`crates/pdfrum-doc/src/ap/mod.rs`):

```rust
for charset in font_map::SUBSTITUTABLE_CHARSETS {
    let Some(dict) = font_map::substitute_font_dict(*charset) else { continue };
    if let Some(font) = load(&dict) {
        substitutes.push((Name::new(font_map::substitute_alias(*charset)), dict, font));
    }
}
```

`substitute_font_dict` **synthesizes** a font dictionary with a 128-entry
`/Differences` array, building 128 `Name` objects through
`adobe_name_from_unicode` — and `load` then constructs a `Font` from that freshly
built dict, which no cache keyed on an `ObjRef` can hit. None of it is
conditional on the document containing a character that needs a second face;
`SUBSTITUTABLE_CHARSETS` is `&[Charset::Hebrew]` and the work is done for every
document regardless.

`annot_render::overlay` calls `FormFonts::load` **per page per render**, and
nothing in `BuildContext` or `RenderCaches` memoizes it — which is precisely why
the warm convention, which rescued the `image` class, does nothing for this.

### 4.3 The excision, on four documents in three classes

The loop was replaced with an empty iterator on HEAD and the same warm A/B
re-run. Milliseconds, whole document, best of five:

| fixture | class | HEAD | HEAD − the loop | M12 committed |
|---|---|---:|---:|---:|
| `shading_coons` | shading | 8.00 | **1.67** | 1.04 |
| `shading_gouraud` | shading | 8.66 | **1.56** | 1.04 |
| `image_bug_718762` | image | 6.06 | **0.69** | 0.48 |
| `forms_number` | forms | 9.81 | **6.40** | 3.22 |

Three of the four land back on their committed baselines within the machine tax
§3 measures. `forms_number` does not, and that is the honest part: the forms
class has a **second**, older cost that `266783f` merely stacked on top of — see
§5.

### 4.4 Why it reached documents that have no form fields

Because the overlay is not the forms class's private code path. Measured with
`--op forms` (§6), the annotation overlay's share of a warm render is:

| fixture | class | overlay share |
|---|---|---:|
| `image_bug_718762` | image | **99.4%** |
| `shading_gouraud` | shading | **89.2%** |
| `shading_coons` | shading | **87.4%** |
| `shading_axial_radial` | shading | 5.8% |
| `vector_paths_1751` | vector | 1.7% |

`shading_coons` and `image_bug_718762` carry annotations, so they pay
`FormFonts::load` on every render exactly as a form document does. A cheap
document with an annotation is the *worst* case for a fixed per-render overlay
cost, which is why `image_bug_718762` — 0.035 ms of actual content — shows the
largest multiple in the whole corpus.

**This also retires the alarming-looking rows in §1's per-fixture spread.**
`image_bug_718762` at "9x worse" and `shading_type4_5` at "5.7x worse" are not
image or shading results at all; they are this one constant divided by a very
small document.

---

## 5. The forms residue that is *not* `266783f`

With the substitute loop gone, `forms_number` is still 6.40 ms against a 3.22 ms
committed baseline, so the forms class has an older cost of its own. The `forms`
op's phase split names it, and it is the same function:

`ap::FormFonts::load`, per render, all pages, `annotations: true` arm:

| fixture | overlay total | **`FormFonts::load`** | `generate_appearances_with_text` | `AnnotList::load` |
|---|---:|---:|---:|---:|
| `forms_list_box` | 91.2 ms | **71.6 ms (78.5%)** | 0.75 ms | 0.30 ms |
| `forms_text_field` | ~78 ms | **78.3 ms** | 0.84 ms | 0.38 ms |
| `forms_number` | 9.81 ms | **9.73 ms (99.2%)** | 0.35 ms | 0.12 ms |

Of that, the `266783f` substitute loop is 30.4 / 34.3 / 12.7 ms — **41%, 44% and
79%**. The remaining ~55% on the two large documents is the *pre-existing* loop
directly above it, which walks the AcroForm `/DR /Font` dictionary and fully
constructs every font in it — encoding tables, `/Differences`, the substitution
ladder — **on every render**. `forms_list_box` and `forms_text_field` are worst
because their `/DR` tables are largest.

**Two things follow, and both matter for what M13 does next.**

`ap::generate_appearances_with_text` is **0.35–0.84 ms**, two orders of magnitude
below the font loading. The intuitive story — "forms is slow because appearance
streams are regenerated every render" — is **wrong**, and measurably so. M12
§10.1 reached the same conclusion by instrumentation on `mixed_formfield` and
recorded it ("appearance *generation* is not the cost… 0.19 ms"); this
measurement confirms it on the forms class proper.

But M12 §10.1 also measured `FormFonts::load` at **1.5 ms** on that document and
concluded it was "under 2% of the render". That was true of `mixed_formfield`,
whose sixteen `/DR` fonts are non-embedded and therefore reached the `OnceLock`
M12 §10.1 itself installed on the built-in faces. It does not generalize: the
forms corpus's fonts are embedded, the `OnceLock` cannot see them, and on
`forms_text_field` the same function costs **78 ms**. **This is the one place
this document contradicts M12** — not its arithmetic, but the scope of a
conclusion drawn from a single unrepresentative document.

---

## 6. What was built to find this: the `forms` op

`benches/src/bin/profile.rs` had four ops — `open`, `render`, `text`, `save` —
and none could see inside the one class M12 named as its largest residue. It now
has a fifth.

**`--op forms` is the annotation appearance overlay, isolated by A/B.** The
definition, and the reason it is this and not the interaction path, is argued at
length in `scripts/profile.nu`'s header; in short: M12's 3.35x is a *render*
ratio taken against `pdfium_test --render-repeats`, and `pdfium_test` never types
into a field, so **not one line of `pdfrum-form` executes on either side of the
number**. An op that looped over `pdfrum_form::apply` would have profiled real
code that contributes nothing to the gap it was built to explain — and would have
missed this finding entirely, because the finding is in `pdfrum-doc`.

What *does* distinguish a forms document on the path both engines run is the
overlay: `pdfium_test --png` seeds `FPDF_ANNOT` and calls `FPDF_FFLDraw` after
every bitmap render, and we do the same through
`pdfrum_doc::annot_render::overlay` under `RenderOptions::annotations`. So the op
runs the same warm render twice — `annotations: true` against `false` —
interleaved round by round in one process, and reports the difference. Both arms
are warm, primed, and best-of-five, for the reasons `bench-oracle.nu` gives for
the same discipline.

It reports a two-row split and a share, and it declines `--sample`, which has no
seam to add to it.

**A `--warm` flag was added at the same time**, because none of the four existing
ops could produce the figure this document needed: the plain `render` loop builds
a fresh `RenderSession` per iteration, which is `render-cold`. `--warm` holds and
primes one, which is `render-warm`, which is the only convention comparable with
the oracle.

### 6.1 A defect in the harness, found and fixed on the way

`--op render`'s clock started before the document was parsed. That was harmless
while every op re-parsed, and stopped being harmless the moment `--warm` put a
*priming render* in front of the loop — a cold render by construction, and on
`image_bug_718762` a cold render is ~1.2 s against a warm one of well under a
millisecond. The giveaway is that the figure fell with the iteration count
instead of converging: **146 ms at n=8, 18 ms at n=100, 11.6 ms at n=400**, one
document, one binary, one machine.

Every number in this document's first sweep was taken with that defect and was
discarded. The clock now starts after setup for every op but `open`, whose
operation is the parse. The trap is written into `scripts/profile.nu`'s header so
the next `--warm` figure that scales with `--iterations` is diagnosed in one
reading rather than bisected.

---

## 7. What to attack first, and what it is worth

**1. Delete the unconditional substitute load — or make it conditional.**
`crates/pdfrum-doc/src/ap/mod.rs`, the `SUBSTITUTABLE_CHARSETS` loop in
`FormFonts::load`. It is loaded for every document and used only by a field that
must write a character its own font cannot, which on this corpus is none of
them. The cheapest correct shape is to build it **on demand** — the generator
already asks `fonts.substitute(charset)` and can be given a lazily-filled slot —
or, failing that, to memoize the synthesized dict and its face in
`BuildContext`, which is where the rest of the per-document font state lives.

*Expected win, measured not estimated:* `shading_coons` 8.00 → 1.67 ms,
`shading_gouraud` 8.66 → 1.56, `image_bug_718762` 6.06 → 0.69, `forms_number`
9.81 → 6.40. Corpus-wide this is most of §1's regression: it should return
`image`, `vector` and `shading` to approximately their M12 figures, and take
`forms` from 7.32x to roughly 4–5x. **This is a defect fix, not an optimization,
and it should land before anything else in M13 is measured** — every other number
in this document is taken over it.

**2. Memoize `FormFonts::load` per document.** With (1) done, the `/DR` font walk
is the whole of the forms residue — 78.3 ms on `forms_text_field`, 71.6 ms on
`forms_list_box`, against a `generate_appearances_with_text` of under 1 ms. The
faces it builds are a pure function of the AcroForm `/DR` dictionary, which does
not change between renders of one document, and `BuildContext` already carries
exactly this kind of state.

*Expected win:* this is 78–99% of the overlay on the forms documents and the
overlay is 93–98% of their render, so the ceiling is most of the class. It is the
difference between `forms` at ~4–5x and `forms` at or near its content cost,
which on `forms_number` is **0.44–0.68 ms against a 1.49 ms oracle**. It is a caching
change, not a kernel rewrite, and it is the single largest measured win available
in the corpus.

**3. Do not start on appearance generation, and do not start on `pdfrum-form`.**
`generate_appearances_with_text` is 0.35–0.84 ms and `AnnotList::load` is
0.10–0.38 ms. Neither is worth touching, and the natural-sounding brief — "the
event cascade, field construction, the appearance stream generation" — points at
the two cheapest things in the profile and at a crate that does not execute on
this path at all.

**4. Re-take §1 on an idle machine.** Its ratios carry a measured 1.5–2x tax that
the oracle column carries only 1.18x of. After (1) lands, the whole table is
worth re-running — and until it is, **no figure in §1 should be quoted as an
engine result**, which is the same warning M12b §10 attached to M12's.

---

## 8. What this does not claim

- **No conformance run was taken.** Nothing here changes a pixel; the `forms` op
  and `--warm` are additions to a profiling binary and the excision in §4.3 was
  measured in a throwaway worktree and never committed. The fix in §7 item 1 will
  need the scoreboard, because `266783f` landed for a reason and a document that
  genuinely needs a Hebrew second face must keep getting one.
- **The `text` and `mixed` classes were not measured**, being outside this
  brief's four. Given §4.4 they are very likely affected in the same way and by
  the same amount, and whoever re-takes §1 should include them.
- **The ratchet was not re-baselined.** It is still the debt `M12b.md` §10 item 1
  and `M12d.md` record, and this document adds a reason not to pay it yet: a
  baseline written over `266783f` would enshrine the regression as the floor.
## 9. The per-fixture spread

Whole document, milliseconds, best of the three backends, **lower is
better**; the ratio is ours over the oracle's, so **> 1 is pdfrum being
slower**. `M12` is `benches/baseline.json`'s committed warm median over
M12 §1.6's oracle column; `now` is this run over this run's oracle.

### `image`

| fixture | M12 | **now** | move | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `image_bug_583804` | 0.89x | **1.12x** | 1.25x | 185.76 | 165.84 |
| `image_bug_718762` | 0.00x | **0.01x** | 9.06x | 4.72 | 596.60 |
| `image_bug_898443` | 0.03x | **0.08x** | 2.73x | 6.74 | 82.24 |
| `image_ccitt_3bigpreview` | 1.94x | **2.45x** | 1.27x | 28.03 | 11.43 |
| `image_ccitt_transfer` | 1.54x | **6.14x** | 4.00x | 15.46 | 2.52 |
| `image_en_fqa` | 11.24x | **2.50x** | 0.22x | 187.28 | 74.90 |
| `image_jbig2_1478366` | 0.02x | **0.13x** | 6.62x | 5.09 | 38.27 |
| `image_jbig2_880920` | 0.22x | **0.68x** | 3.06x | 8.47 | 12.46 |
| `image_jpx_123` | 0.65x | **1.41x** | 2.17x | 11.39 | 8.09 |
| **geomean** | **0.24x** | **0.53x** | **2.26x** | | |

Spread of the `now` column: **0.01x .. 6.14x**.

### `vector`

| fixture | M12 | **now** | move | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `vector_en_system` | 2.84x | **1.90x** | 0.67x | 123.78 | 65.19 |
| `vector_en_tem` | 1.32x | **5.84x** | 4.41x | 46.83 | 8.02 |
| `vector_font_feature` | 2.65x | **5.61x** | 2.11x | 116.38 | 20.75 |
| `vector_font_size14` | 1.49x | **4.21x** | 2.82x | 60.40 | 14.34 |
| `vector_paths_1751` | 0.51x | **0.90x** | 1.77x | 20.32 | 22.51 |
| `vector_tcpdf_009` | 0.07x | **0.26x** | 3.65x | 12.27 | 48.06 |
| **geomean** | **0.90x** | **1.98x** | **2.20x** | | |

Spread of the `now` column: **0.26x .. 5.84x**.

### `shading`

| fixture | M12 | **now** | move | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `shading_axial_radial` | 0.55x | **0.82x** | 1.48x | 46.31 | 56.59 |
| `shading_coons` | 1.12x | **5.72x** | 5.10x | 8.00 | 1.40 |
| `shading_gouraud` | 1.04x | **5.88x** | 5.65x | 8.66 | 1.47 |
| `shading_tcpdf_030` | 0.56x | **1.01x** | 1.81x | 82.65 | 81.61 |
| `shading_tcpdf_056` | 1.30x | **4.46x** | 3.44x | 10.07 | 2.25 |
| `shading_tcpdf_058` | 1.29x | **3.90x** | 3.02x | 22.31 | 5.73 |
| `shading_tensor` | 0.27x | **0.52x** | 1.91x | 19.14 | 36.63 |
| `shading_type4_5` | 3.97x | **22.78x** | 5.74x | 6.49 | 0.28 |
| **geomean** | **0.95x** | **2.95x** | **3.11x** | | |

Spread of the `now` column: **0.52x .. 22.78x**.

### `forms`

| fixture | M12 | **now** | move | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `forms_combo_box` | 5.44x | **12.09x** | 2.22x | 50.91 | 4.21 |
| `forms_list_box` | 6.87x | **15.57x** | 2.27x | 88.87 | 5.71 |
| `forms_number` | 3.14x | **8.13x** | 2.59x | 12.09 | 1.49 |
| `forms_push_button` | 0.39x | **0.69x** | 1.79x | 47.16 | 68.11 |
| `forms_signature` | 3.84x | **11.42x** | 2.98x | 25.51 | 2.23 |
| `forms_text_field` | 5.83x | **9.92x** | 1.70x | 48.50 | 4.89 |
| `forms_widgets_407` | 4.67x | **9.36x** | 2.01x | 56.02 | 5.98 |
| **geomean** | **3.35x** | **7.32x** | **2.18x** | | |

Spread of the `now` column: **0.69x .. 15.57x**.


**One row moved the right way and is worth naming:** `image_en_fqa` goes from
11.24x — M12 §11's named worst warm row in the corpus — to **2.50x**, a 0.22x
move. That is M12d's D3 landing, and it is the only improvement in the four
classes. M12 §11 called it "the proof" that single-image documents were where
the engine was genuinely behind; it is substantially less behind.


---

## 10. The fix, and the table re-taken — on a box that never went idle

**Taken 2026-09-02, after §7 items 1 and 2 landed as one change.** Everything above this line is left exactly
as it was written. §1 and §9's figures were taken at a load average of 47–65 on
32 cores and §3 says so; they are **not** deleted or corrected in place, because
they are the measurement that motivated the fix and the reason it was found. This
section adds the post-fix numbers beside them.

### 10.1 What was changed

`ap::FormFonts::load` is now **memoized on `BuildContext`**, keyed on the
catalog's `/AcroForm`, and returns `Arc<FormFonts>`.

§7 recommends making the substitute load lazy first (item 1) and memoizing
second (item 2). Measured on this machine, **laziness alone is the smaller
half**: replacing the `SUBSTITUTABLE_CHARSETS` loop with an empty iterator —
§4.3's excision, re-run on this box — takes `forms_text_field`'s overlay from
38.3 ms to 26.8 ms and `forms_number`'s from 7.20 to 3.20. The `/DR` walk §5
identifies as the older, larger half is untouched by it. Memoizing subsumes
laziness, so that is what landed, and item 1 is closed by item 2 rather than
separately.

**It also does something §7 did not anticipate, and this is the part worth
recording.** The loop is only one of the three per-call costs §7 lists, and the
two documents §1 shows worst — `image_bug_718762` and `shading_coons` — **have
no `/AcroForm` at all**. Their `/DR` walk finds nothing; their entire bill is the
unconditional fallback load and the synthesized substitutes. A cache keyed on the
form reference alone would have missed exactly the case that made this a
whole-corpus regression rather than a forms one. So the key names three cases:

| catalog's `/AcroForm` | cached | why |
|---|---|---|
| an indirect reference | under that reference | the document-scoped identity every other `BuildContext` cache keys on |
| absent | one shared slot | with no form the faces depend on **nothing** document-specific — the fallback dict and the synthesized substitutes are constants |
| a direct dictionary | not cached | no identity to key on, and its content *is* document-specific |

The cache lives on `BuildContext`, beside `font_instances`, `colorspaces`,
`functions` and `images`, and is keyed for the reason those are: one context may
legitimately be threaded through two documents. `Page::render_with`'s contract —
"thread one context through them all" — is what now covers form fonts too;
putting the cache on `RenderSession` would have left that promise unpaid for
every `render_with` caller.

`pdfrum-page` is below `pdfrum-doc` and cannot name `FormFonts`, so the slot
stores `Arc<dyn Any + Send + Sync>` and the layer above supplies the type. That is
storage erasure, not a polymorphism seam — nothing is dispatched through the
`Any`, it is downcast straight back to the one type that put it there — so
STYLE.md §2b's closed list of three trait seams is untouched.

### 10.2 The invariants at `ap/mod.rs`'s comments are preserved, deliberately

The two comments the brief flags were **not** changed, and neither was the
behaviour they describe:

- **The fallback still goes through the same loader**, not the stock-metrics
  constructor, because the ascent and descent must come from the face actually
  substituted — 905/−211 against the base-14 tables' 718/−219, which on a list
  box is the row pitch and two extra rows in a thirty-unit box.
- **The second faces are still loaded in `load`**, not lazily where a field
  discovers it needs one, because loading needs the page's font cache and the
  generators are pure functions of the faces handed to them.

Nothing about *what* is built changed — **only how many times**. That is what
makes the appearance streams identical rather than merely equivalent, and it is
why the lazy shape §7 preferred was not taken: laziness would have had to move
the substitute load out from under the page's font cache, which is precisely
what the comment at that line forbids.

### 10.3 Conformance: nothing moved

§8 warned that a fix here would need the scoreboard, "because `266783f` landed
for a reason and a document that genuinely needs a Hebrew second face must keep
getting one." It does.

**1757 rows, 1512 pass, 245 fail — before and after, identical.** Every tag
bucket identical (`form-events` 8, `js-transcript` 33, `page-count` 2,
`pixel-fail` 43, `tierA-mismatch` 170). Stronger than the totals: **all 1757
rows' Tier-A mismatch lists are byte-identical**, so not one generated appearance
stream moved anywhere in the corpus. The eight `form-events` rows are unchanged.
Four unit tests pin the three key cases and the two-document one.

### 10.4 The machine, again — and why this table is *not* labelled idle

§3 asked for §1 to be re-taken on an idle box, and §7 item 4 repeats it. **That
was attempted for the whole of this session and could not be delivered**, and
saying so is more useful than relabelling a loaded run.

Load was sampled every two minutes from 17:49 to the close of this run. It began
at **57.9** (1-minute) / 61.0 (5-minute) and never fell below **30.4**. The floor
is not this work: in the 17:54–18:00 window, with nothing of this session's own
running, the box still read **30.8, 32.3, 42.8, 30.4**. The occupants are the
same ones §3 names — two `python3` training jobs at 473% and 445% CPU (elapsed
**16 h 27 m** and 1 h 20 m at the time of measurement) and twelve `osmium`
processes belonging to another user. Neither is this session's to stop.

So the honest statement is: **§1's ratios were taken at load 47–65, and §10.5's
at load 30–66 — lower, overlapping, and still not idle.** §10.5's absolute
milliseconds inherit a machine tax of the same kind §3 measured, and the
asymmetry §3 found — ~1.18x on the oracle's side against ~1.5–2x on ours —
applies to them too. **They remain upper bounds**, and §3's instruction to
re-take them on a quiet box is *not* discharged by this section; it is carried
forward with a measurement of why.

**What does not depend on the machine is §10.5's before/after column**, and that
is the column the fix is judged on. It is an interleaved A/B — both binaries, both
arms, round by round, on one document in one stretch of minutes, each arm's
minimum kept — so a load excursion lands on both arms rather than one. §4.3 used
exactly this discipline for the same reason, and §3 says why it is
load-independent even where the absolute ratios are not.

### 10.5 The table

Whole document, milliseconds, best of the three backends over three interleaved
rounds, **lower is better**; the ratio is ours over the oracle's, so **> 1 is
pdfrum being slower**. `M12` is `benches/baseline.json`'s committed warm median
over M12 §1.6's oracle column; `under load` is §9's column; `now` is this run's
post-fix figure over this run's oracle, taken by `scripts/bench-oracle.nu`'s
method at `n=21`, best of 5, on the same binary and the same 30 files.

*On the script's name:* §2 and this section were measured while `scripts/` was
still bash, and PR #1 has since translated every script to nushell — the
references above and throughout this document now name the `.nu` files, which
are what exist. The oracle method is unchanged by that translation and was
checked rather than assumed: `bench-oracle.nu` runs the same
`(t[n] − t[1]) / (n − 1)` marginal-pass formula against the same
`--md5 --render-repeats` invocation, takes the same minimum-of-rounds rather
than a mean, and carries the same defaults. The figures below were produced by
that method restricted to the four classes' 30 files, with the PDFs copied to a
scratch directory first because `pdfium_test` writes beside its input and the
oracle tree is read-only.

| class | M12 §1.10 | under load (§1) | **now, fixed** | vs M12 | vs under-load |
|---|---:|---:|---:|---:|---:|
| `image` | 0.24x | 0.53x | **0.09x** | 0.38x | 0.17x |
| `vector` | 0.90x | 1.98x | **0.78x** | 0.87x | 0.39x |
| `shading` | 0.95x | 2.95x | **0.86x** | 0.90x | 0.29x |
| `forms` | 3.35x | 7.32x | **2.16x** | 0.64x | 0.29x |

**All four class geomeans are better than M12's**, not merely recovered to
them, and the whole-corpus geomean over the four goes from §1's **2.02x** to
**0.53x** — against M12's own 0.83x. Three things to read carefully before that
is quoted:

- **A class geomean beating M12 does not mean every row did.** Eight of the
  thirty are above their M12 figure and §10.6 names all eight; the geomeans are
  below M12's anyway because the rows that improved improved by more.

- **`forms` at 2.16x is the first movement on M12 §11's named residue since M12
  named it.** M12's 3.35x had never been re-measured (`M12b.md` §10 item 2,
  `M12d.md` at close); it is now 0.64x of what it was. §7 item 2 predicted "at
  or near its content cost" as the ceiling and this is not yet that — see §10.6's
  `forms_combo_box` row.
- **This is still not an idle box** (§10.4). The absolute milliseconds on both
  sides carry a machine tax, and §3 measured that tax as asymmetric — ~1.18x on
  the oracle, ~1.5–2x on ours. If that asymmetry still holds, these ratios are
  *upper bounds* and the true figures are better again. That cuts the same way
  it did in §1, which is the one respect in which nothing has changed.

### 10.6 The per-fixture spread

### `image`

| fixture | M12 | under load | **now** | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `image_bug_583804` | 0.89x | 1.12x | **0.79x** | 160.31 | 204.16 |
| `image_bug_718762` | 0.00x | 0.01x | **0.00x** | 0.02 | 708.50 |
| `image_bug_898443` | 0.03x | 0.08x | **0.02x** | 1.99 | 87.66 |
| `image_ccitt_3bigpreview` | 1.94x | 2.45x | **1.66x** | 22.18 | 13.34 |
| `image_ccitt_transfer` | 1.54x | 6.14x | **0.26x** | 0.88 | 3.34 |
| `image_en_fqa` | 11.24x | 2.50x | **2.03x** | 146.76 | 72.33 |
| `image_jbig2_1478366` | 0.02x | 0.13x | **0.00x** | 0.16 | 34.87 |
| `image_jbig2_880920` | 0.22x | 0.68x | **0.19x** | 3.06 | 15.70 |
| `image_jpx_123` | 0.65x | 1.41x | **0.83x** | 9.68 | 11.61 |
| **geomean** | **0.24x** | **0.53x** | **0.09x** | | |

Spread of the `now` column: **0.00x .. 2.03x**.

### `vector`

| fixture | M12 | under load | **now** | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `vector_en_system` | 2.84x | 1.90x | **1.08x** | 89.64 | 82.95 |
| `vector_en_tem` | 1.32x | 5.84x | **0.62x** | 9.03 | 14.51 |
| `vector_font_feature` | 2.65x | 5.61x | **3.42x** | 119.66 | 35.02 |
| `vector_font_size14` | 1.49x | 4.21x | **3.70x** | 65.00 | 17.58 |
| `vector_paths_1751` | 0.51x | 0.90x | **0.44x** | 10.68 | 24.42 |
| `vector_tcpdf_009` | 0.07x | 0.26x | **0.06x** | 2.90 | 48.28 |
| **geomean** | **0.90x** | **1.98x** | **0.78x** | | |

Spread of the `now` column: **0.06x .. 3.70x**.

### `shading`

| fixture | M12 | under load | **now** | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `shading_axial_radial` | 0.55x | 0.82x | **0.55x** | 36.84 | 67.42 |
| `shading_coons` | 1.12x | 5.72x | **0.57x** | 0.85 | 1.50 |
| `shading_gouraud` | 1.04x | 5.88x | **0.76x** | 0.74 | 0.97 |
| `shading_tcpdf_030` | 0.56x | 1.01x | **0.49x** | 53.74 | 109.88 |
| `shading_tcpdf_056` | 1.30x | 4.46x | **0.74x** | 2.43 | 3.27 |
| `shading_tcpdf_058` | 1.29x | 3.90x | **1.88x** | 12.72 | 6.78 |
| `shading_tensor` | 0.27x | 0.52x | **0.42x** | 16.74 | 39.81 |
| `shading_type4_5` | 3.97x | 22.78x | **4.38x** | 1.26 | 0.29 |
| **geomean** | **0.95x** | **2.95x** | **0.86x** | | |

Spread of the `now` column: **0.42x .. 4.38x**.

### `forms`

| fixture | M12 | under load | **now** | ours (ms) | oracle (ms) |
|---|---:|---:|---:|---:|---:|
| `forms_combo_box` | 5.44x | 12.09x | **8.16x** | 36.10 | 4.42 |
| `forms_list_box` | 6.87x | 15.57x | **5.38x** | 31.61 | 5.88 |
| `forms_number` | 3.14x | 8.13x | **1.83x** | 1.99 | 1.08 |
| `forms_push_button` | 0.39x | 0.69x | **0.20x** | 13.45 | 68.08 |
| `forms_signature` | 3.84x | 11.42x | **2.58x** | 6.45 | 2.50 |
| `forms_text_field` | 5.83x | 9.92x | **2.06x** | 10.41 | 5.05 |
| `forms_widgets_407` | 4.67x | 9.36x | **2.57x** | 15.54 | 6.05 |
| **geomean** | **3.35x** | **7.32x** | **2.16x** | | |

Spread of the `now` column: **0.20x .. 8.16x**.

**All thirty rows are better than their §9 figure. Twenty-two of the thirty are
also better than their M12 figure**, and the eight that are not must be named
rather than averaged away — the class geomeans are all below M12's, so an
unqualified "every class beat M12" would hide these:

| row | M12 | now | its §10.7 speedup |
|---|---:|---:|---:|
| `forms_combo_box` | 5.44x | **8.16x** | 1.86x |
| `vector_font_size14` | 1.49x | **3.70x** | 1.27x |
| `vector_font_feature` | 2.65x | **3.42x** | 1.12x |
| `shading_tcpdf_058` | 1.29x | **1.88x** | 1.33x |
| `shading_type4_5` | 3.97x | **4.38x** | 6.24x |
| `image_jpx_123` | 0.65x | **0.83x** | 1.03x |
| `shading_tensor` | 0.27x | **0.42x** | 2.39x |
| `image_bug_718762` | 0.00x | **0.00x** | 206.26x |

Seven of the eight have a §10.7 speedup at or below **2.4x** against a
corpus geomean of 3.13x — they are the documents where the removed constant was
the *smallest* share, so what is left in them is the machine (§10.4) and whatever
each was already carrying, not this regression. `image_bug_718762` is in the list
only as a rounding artefact: 0.023 ms over 708 ms is 0.00003x, below the two
decimals this table prints, and it is the corpus's largest improvement.

**`forms_combo_box` is the one that is not explained that way, and it is now the
worst row in the corpus.** Its speedup, 1.86x, is the lowest in its class against
a class geomean of 3.36x, which says the cost left in it is **not** the one this
fix removed. Its `--op forms` split is the next thing to take, not another cache
— and §7 item 3's warning still stands: `generate_appearances_with_text` was
0.35–0.84 ms and `AnnotList::load` 0.10–0.38 ms, so whatever this is, it is a
third thing neither §5 nor this section has isolated.

At the other end, `forms_push_button` at **0.20x** is the fastest relative row in
the corpus (0.39x at M12), on a document where the oracle takes 68 ms.

The pattern across all thirty is one shape: the fix removed a **fixed per-render
cost**, so it helped most where the document was cheapest and least where the
document was already expensive. That is why `image` moves 0.53x → 0.09x while
`vector` only moves 1.98x → 0.78x, and it is the same explanation §4.4 gave for
why the regression looked worst on the cheapest documents.

### 10.7 Before and after the fix, per fixture

Both binaries, interleaved round by round on one document in one stretch of
minutes, each arm's minimum over three rounds × three backends kept. **Every one
of the thirty fixtures improved**, which is itself the finding: the cost removed
was a *constant per render*, not something proportional to the work, so the
speedup is largest exactly where the document is cheapest.

| class | pre-fix geomean (ms) | post-fix geomean (ms) | speedup | spread |
|---|---:|---:|---:|---|
| `image` | 16.73 | **3.60** | **4.65x** | 1.01x .. 206.26x |
| `vector` | 40.50 | **24.08** | **1.68x** | 1.04x .. 4.21x |
| `shading` | 16.52 | **5.48** | **3.01x** | 1.13x .. 9.60x |
| `forms` | 39.64 | **11.80** | **3.36x** | 1.86x .. 4.53x |
| **all 30** | **24.33** | **7.77** | **3.13x** | 1.01x .. 206.26x |

The three fixtures §4.3 excised the loop on, and the three `forms` documents §5
profiled:

| fixture | pre-fix | **post-fix** | speedup | §4.3's "loop excised" | M12 committed |
|---|---:|---:|---:|---:|---:|
| `shading_coons` | 7.12 | **0.85** | 8.41x | 1.67 | 1.04 |
| `shading_gouraud` | 7.13 | **0.74** | 9.60x | 1.56 | 1.04 |
| `image_bug_718762` | 4.74 | **0.02** | 206.26x | 0.69 | 0.48 |
| `forms_list_box` | 89.80 | **31.61** | 2.84x | — | — |
| `forms_text_field` | 45.71 | **10.41** | 4.39x | — | — |
| `forms_number` | 8.01 | **1.99** | 4.03x | 6.40 | 3.22 |

**All three of §4.3's documents now land *below* their M12 committed baselines**,
on a machine carrying load 30–66 where those baselines were taken quieter. §4.3
predicted 1.67 / 1.56 / 0.69 for the excision; memoizing beats that by a further
2–30x, because the excision removed one of three costs and this removes all
three. `forms_number` is 1.99 against §4.3's 6.40 and M12's 3.22 — so §5's "older
cost that `266783f` merely stacked on top of", the `/DR` walk, is paid too.

### 10.8 What this contradicts, and what it confirms

**Confirmed, in full:** §4's diagnosis (one commit, one function, most of the
corpus), §4.4's explanation of why documents with no form fields paid it,
§5's finding that the `/DR` walk is a second and larger cost than the substitute
loop, §5's correction of M12 §10.1's "under 2%" (that conclusion did not
generalize, and this fix is the proof of what it was hiding), and §6.1's harness
defect — which was **not** on `main` and was cherry-picked (`9cbc347`) rather
than reproduced, so this document's two commits and the fix now share one
history.

**One thing this document under-stated.** §7 item 1 describes the substitute loop
as the thing to remove and item 2 as the follow-up. On the corpus the ordering is
the other way round for *reach*: item 2 subsumes item 1, and item 1 alone would
have left the two worst rows in §1 — `image_bug_718762` and `shading_coons`,
both of which have **no `/AcroForm`** — paying the fallback load on every render.
§7's own expected-win table anticipated their post-fix values as 0.69 and 1.67 ms;
they are **0.02 and 0.85**. The estimate was right about the mechanism and low
about the size, because it costed the excision rather than the cache.

**Nothing here contradicts §1's arithmetic, §3's machine analysis, or §9's
per-fixture spread.** They were measured under load and they say so; §10.4 finds
the same box under the same tenants and does not claim otherwise.
