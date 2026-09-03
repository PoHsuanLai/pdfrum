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

---

## 11. The third cost: a clip push, and the two device-sized passes it took

**Taken 2026-09-03, on the same box, at load 29–49.** §10.6 closed by naming
`forms_combo_box` as the corpus's worst row at **8.16x**, observing that its
1.86x speedup was the lowest in its class against a class geomean of 3.36x —
"which says the cost left in it is **not** the one this fix removed" — and
instructing that "its `--op forms` split is the next thing to take, not another
cache". This section is that split, and it found the third thing §10.6 said
neither §5 nor §10 had isolated.

**It is not in `pdfrum-doc` at all. It is in the rasterizer, and it is a clip.**

### 11.1 Where the split actually landed

`--op forms` puts **87.9%** of a warm `forms_combo_box` render in the overlay
(15.79 ms of 17.97). That is where §10.6 pointed and it is correct. What it does
not say is which half of the overlay, and the answer is neither of the two §5
ruled out nor the one §7 item 2 already fixed:

| overlay phase | `forms_combo_box`, per warm render |
|---|---:|
| `AnnotList::load` | 0.10 ms |
| `ap::FormFonts::load` (post-`e084fbd`) | **0.002 ms** |
| `ap::generate_appearances_with_text` | 0.20 ms |
| `nav::hidden_by_open_action` | 0.000 ms |
| the per-annotation `build_form_object_with` loop | 0.84 ms |
| **the page graph, rasterized** | **6–12 ms per page** |

The overlay's *build* is about a millisecond for both pages together, and
`FormFonts::load` is two microseconds — §7 item 2's cache doing exactly what
§10.1 said it would. **The residue is in drawing what the build produced**, and
the overlay's contribution to the drawing is 37 objects on page 1 and 25 on
page 2 against a page graph of 155 that the document's own content costs
**1.7 ms** to draw.

### 11.2 What named it, and what could not

`perf` was unavailable (`perf_event_paranoid` is 4 on this box and there is no
`sudo`), so the naming was done with the gated instrument the repository
already has.

**`--sample` cannot see this cost, and that is a property of the op rather than
a defect.** `timed_render` hoists the page graph with `page.objects()`, which is
the page's *content* — `Page::render_with` is the only path that runs
`annot_render::overlay`, so a `--sample` run reports `forms_combo_box` at 1.7 ms
and attributes nothing to the overlay. The figure is right about what it
measures and is not the render the oracle is divided into. `scripts/profile.nu`'s
header already warns that `--sample` and `--warm` are not interchangeable
hoists; this is a third case of it, and the one that would have hidden the
finding entirely.

**`--walk` on the warm loop is what named it**, by elimination:

| phase | ms/iter | of ENGINE | calls/iter |
|---|---:|---:|---:|
| glyphs | 1.59 | 8.7% | 205 |
| path prep | 1.57 | 8.6% | 225 |
| clip | 0.10 | 0.6% | 468 |
| cull | 0.07 | 0.4% | 468 |
| color | 0.05 | 0.3% | 429 |
| **INTERPRETATION** | **16.52** | **90.1%** | dispatch, state, recursion |
| ENGINE | 18.33 | 100.0% | |

Every phase the instrument names is under 1.6 ms, and 90% of the render is in
the bucket that is defined as *what is left*. That is the shape a cost has when
it sits **below** the walk rather than inside it: `Phase::Clip` times
`clip::resolve`, which decides what to push, and stops at the device call that
pushes it. Timing that call directly put **8.2 ms** in
`AggDevice::push_clip_mask` and a further **2.0 ms** in the `coverage_of` that
feeds it — together **56%** of the render, on 250 clip pushes per iteration.

The 250 are not an accident of this file. An annotation appearance is a form
XObject, `pdfrum_page::build_form_object_with` pushes its `/BBox` as a clip
(the comment there says why: "an appearance reached from an annotation has no
enclosing anything, so nothing else would ever apply it"), and every widget on
the page contributes one — plus whatever the appearance's own content pushes
inside it.

### 11.3 The cost, and why it is a defect rather than the price of a clip

`AggDevice`'s clip is a coverage plane the size of the **device** — 595 × 841 on
this document, half a megabyte — because `Target` indexes it by absolute device
row and column. That is correct and is not what this changes. What each push did
with it was:

1. `coverage_of` allocated a fresh device-sized plane and swept the path into it.
2. `push_clip_mask` called `AlphaMask::intersect`, a pass over **all** 500 000
   bytes, to fold in the plane below.
3. `sync_clip` **cloned** the finished plane into the target — a half-megabyte
   `memcpy` — and did the same again on every `pop`.

For a combo box's `/BBox`, which is about thirty rows tall, steps 2 and 3 are
roughly 1.5 MB of memory traffic to compute and install thirty rows of answer,
two hundred and fifty times per render. That is the pathology the brief
predicted in kind — "a clip/transparency group allocated per widget" — though
not in place: the transparency-group path is never reached on this document
(`render_grouped` is not entered once), and the per-widget allocation is the
clip's.

### 11.4 The fix, and the invariant each half preserves

Both halves are inside `pdfrum-raster-agg`. **Neither changes the arithmetic**,
which is the point: `CFX_AggClipRgn::IntersectMask`'s truncating `a * b / 255`
is what makes a clipped edge land where the oracle's does, and it is untouched.

**The band.** `coverage_of` now reports the half-open range of rows its sweep
actually wrote, and `push_clip_mask` intersects over that range alone. The
equivalence is exact rather than approximate: the plane is allocated zero, the
sweep touches only the band, and `mul255(0, b) == 0` for every `b` — so every
byte outside the band is *already* the product a whole-buffer pass would have
written. The band is folded from the writes rather than read off the first and
last callback, because a span whose columns all fall outside the buffer writes
nothing and would otherwise widen it.

**The share.** The clip stack and the target now hold the same plane behind an
`Arc`. Nothing mutates a clip once it is on the stack — an intersection builds a
*new* plane from the incoming coverage — so the share is of an immutable value
and `sync_clip` becomes a refcount bump. This is the half that pays on `pop` as
well as on `push`, which the band cannot help with because a pop computes
nothing.

Three tests pin what the change trades on, and each targets one way it could be
wrong: `a_clip_outside_the_previous_ones_rows_paints_nothing` (two clips whose
row bands are disjoint, so the answer is entirely outside the inner band),
`a_banded_intersection_still_carries_the_outer_clips_columns` (the outer clip's
*columns* must reach the inner plane's rows — a band that dropped them would
leave a whole stripe visible), and
`each_pop_hands_the_target_the_plane_one_level_out` (a four-deep stack checked
at every level on the way out, because a share that lagged by one would still
pass a single pop).

**Nothing public moves.** `Target` lives behind a private `mod` and `AggDevice`'s
`clips` is a private field, so the `Arc` is invisible outside the crate;
`scripts/api-snapshot.nu` reports the surface matching the committed baseline.
`AlphaMask::intersect` in `pdfrum-render` is unchanged and still public — the
banded spelling is a private function in `pdfrum-raster-agg`, so no API grew to
serve one caller.

**Only the AGG backend is changed.** `pdfrum-raster-tinyskia` has a clone of the
same shape in its own `push_clip`/`push_clip_rect`, but it has no `sync_clip`
— it reads the stack top directly — so it pays one copy where AGG paid three
passes, and `intersect_path` is `tiny-skia`'s rather than ours. AGG is the
backend every figure in this document is taken on, and widening the change to a
backend the corpus is not measured on would be an unmeasured edit; it is left as
a noted, smaller instance of the same shape.

### 11.5 The machine

**Load 29–49 on 32 cores**, sampled per fixture and reported in the table's last
column. §10.4's tenants are still there and §3's warning still applies to every
absolute millisecond below: they are upper bounds and the asymmetry §3 measured
(~1.18x on the oracle's side, ~1.5–2x on ours) has not been re-measured.

**What does not depend on the machine is the before/after column.** It is an
interleaved A/B in §10.4's discipline and for §10.4's reason — the pre-fix
binary, the post-fix binary and the oracle, round by round on one document in
one stretch of minutes, each arm's minimum kept, so a load excursion lands on
all three arms rather than one. `n = 21`, best of five, `--op render --warm`,
the oracle by `bench-oracle.nu`'s `(t[n] − t[1]) / (n − 1)` marginal-pass
formula against the same `--md5 --render-repeats` invocation, with the PDFs
copied to scratch first because `pdfium_test` writes beside its input and the
oracle tree is read-only.

### 11.6 The table

Whole document, milliseconds, AGG, **lower is better**; the ratio is ours over
the oracle's, so **> 1 is pdfrum being slower**. `before` is `5407319`'s parent,
`after` is `5407319`.

| fixture | before (ms) | **after (ms)** | speedup | oracle (ms) | ratio before | **ratio after** | load |
|---|---:|---:|---:|---:|---:|---:|---:|
| `forms_combo_box` | 16.46 | **6.72** | **2.45x** | 4.38 | 3.76x | **1.54x** | 30.0 |
| `forms_list_box` | 26.39 | **13.04** | 2.02x | 5.58 | 4.73x | **2.34x** | 29.1 |
| `forms_number` | 2.16 | **1.73** | 1.25x | 0.89 | 2.42x | **1.94x** | 35.0 |
| `forms_push_button` | 10.62 | **6.24** | 1.70x | 67.47 | 0.16x | **0.09x** | 41.0 |
| `forms_signature` | 6.12 | **2.24** | 2.73x | 1.65 | 3.71x | **1.36x** | 35.0 |
| `forms_text_field` | 13.59 | **8.05** | 1.69x | 2.59 | 5.24x | **3.10x** | 41.2 |
| `forms_widgets_407` | 20.51 | **9.14** | 2.24x | 8.10 | 2.53x | **1.13x** | 45.9 |
| **`forms` geomean** | | | | | **2.29x** | **1.17x** | |
| | | | | | | | |
| `vector_paths_1751` | 13.35 | 14.14 | 0.94x | 24.39 | 0.55x | 0.58x | 46.8 |
| `image_bug_583804` | 179.94 | 181.84 | 0.99x | 181.49 | 0.99x | 1.00x | 36.3 |
| `shading_axial_radial` | 37.27 | 36.40 | 1.02x | 48.95 | 0.76x | 0.74x | 48.8 |

**The three controls are flat — 0.94x, 0.99x, 1.02x, which is this box's noise
floor and not a measurement of anything.** That is the shape this fix should
have: a page whose clip stack is shallow pushes few clips, and a push that was
never the cost cannot become cheaper. It is the opposite of §10.6's pattern,
where the removed cost was *fixed per render* and so helped the cheap documents
most; this one is per clip push, and it helps in proportion to how many a
document makes.

**Every forms row improves, and `forms_combo_box` improves most.** It goes from
the worst row in its class to the fourth of seven, and its 2.45x speedup is the
class's largest — which is the confirmation that the cost §10.6 could not
account for is the one this removes. The class geomean against the oracle goes
**2.29x → 1.17x**.

The mechanism is visible in `--op forms` on the other two documents with many
widgets, where the overlay's *share* falls while the page content underneath it
does not move at all — which is what a fix to the overlay's drawing looks like
and what a fix to the page's own content would not:

| fixture | overlay share, before | overlay share, after | page content, before | page content, after |
|---|---:|---:|---:|---:|
| `forms_list_box` | 92.9% | 83.7% | 1.94 ms | 2.00 ms |
| `forms_widgets_407` | 98.4% | 96.8% | 0.24 ms | 0.25 ms |

*On comparing this with §10.5's 2.16x:* these are not the same measurement and
should not be subtracted. §10.5 is `bench-oracle.nu` over 30 files with the
three backends' best; this is one backend over the class's seven, on a different
day at a different load, with a `before` column that is this branch's parent
rather than M12's. The **before** column is the honest comparand for the
**after** one, and the pair is the claim.

### 11.7 What this does not claim

- **Not an idle box.** §3's instruction is still undischarged, for the fourth
  time in this document, and for the same reason: the tenants are not this
  session's to stop.
- **`forms_text_field` at 3.10x is now the class's worst row** and is not
  explained here. Its speedup, 1.69x, is among the class's lowest, so whatever
  is left in it is — by §10.6's own argument — a *fourth* cost rather than this
  one. Its `--op forms` split is the next thing to take.
- **The other three classes were not re-measured**, only spot-checked by the
  three controls. A document with a deep clip stack outside the forms class
  would gain here, and none was looked for.
- **`tiny-skia` and `vello_cpu` are unchanged** (§11.4), so a figure taken on
  either still carries the old cost and must not be compared with a row above.
- **The ratchet is still not re-baselined.** §8's third bullet stands, and
  `benches/baseline.json` is untouched.

## 12. The fourth cost: the clip plane's *allocation*, and the pool that removes it

**Taken 2026-09-03, on the same box, at load 43–131.** §11.7 named
`forms_text_field` as the class's worst row at **3.10x**, observed that its
1.69x speedup was among the class's lowest, and concluded that whatever was
left in it "is — by §10.6's own argument — a *fourth* cost rather than this
one". This section is that split. It found the fourth cost, and it is the
half of `push_clip_rect` that §11 did not remove.

**§11 made the clip's intersection cheap and left its allocation alone.**

### 12.1 Where the split landed, and why §11's method had to be re-run

`--op forms` on `forms_text_field` reports **67.6%** of a warm render in the
*page content* and 32.4% in the overlay — the reverse of `forms_combo_box`'s
87.9/12.1, and the first thing that says this row is not `forms_combo_box`'s
residue in a smaller document:

| fixture | page content | annotation overlay |
|---|---:|---:|
| `forms_text_field` | **67.6%** (9.57 ms) | 32.4% (4.60 ms) |
| `forms_signature` | 72.0% | 28.0% |
| `forms_combo_box` | 29.9% | 70.1% |
| `forms_number` | 26.2% | 73.8% |

That reversal is real but it is not the finding, and reading it as one would
have sent this section into `pdfrum-page`. **The overlay's objects are drawn
through the same device calls the page's own are**, so a cost in the *drawing*
shows up on whichever side happens to own more objects — and `--op forms`
attributes by arm, not by function.

`--sample` again could not see it, for §11.2's reason and one more: it hoists
`page.objects()` and so reports `forms_text_field`'s page content at **1.97 ms**
against the warm loop's 9.57. `--walk` on the warm loop is not reachable either
— the walk report is emitted from `timed_render`, which is the `--sample` path.
So the naming was done as §11 did it, with direct `Instant` pairs, but placed
one level lower: around `Page::paint`'s three phases, and then around each
`RenderDevice` call inside `AggDevice`.

### 12.2 What that named

`Page::paint`, per warm render, all pages, `annotations: true`:

| fixture | whole | `build` (graph) | `annot_render::overlay` | `render_page_with` |
|---|---:|---:|---:|---:|
| `forms_text_field` | 6.63 ms | 0.73 (11.0%) | 1.17 (17.6%) | **4.66 (70.3%)** |
| `forms_combo_box` | 9.94 ms | 0.73 (7.3%) | 1.74 (17.5%) | **7.36 (74.1%)** |
| `forms_number` | 1.96 ms | 0.22 (11.2%) | 0.61 (31.1%) | **1.12 (57.1%)** |

The build is a tenth and the overlay's *build* is a fifth; the rasterizer is
seventy per cent, which is where §11 left it. Inside `AggDevice`:

| call | `forms_text_field` | `forms_combo_box` | `forms_number` |
|---|---:|---:|---:|
| `push_clip_rect` — `coverage_of` | 0.95 ms / 133 | 2.51 ms / 276 | 0.28 ms / 40 |
| &nbsp;&nbsp;of which **`AlphaMask::new`** | **0.61 ms** | **1.55 ms** | **0.19 ms** |
| &nbsp;&nbsp;of which the sweep | 0.26 ms | 0.77 ms | 0.07 ms |
| &nbsp;&nbsp;of which `add_path` | 0.06 ms | 0.13 ms | 0.01 ms |
| `push_clip_mask` (post-30c0419) | 0.12 ms | 0.40 ms | 0.002 ms |
| `pop` | 0.007 ms | 0.02 ms | 0.002 ms |
| `fill_path` | 0.95 ms / 74 | 1.70 ms / 222 | 0.49 ms / 40 |
| `draw_image` | 0.45 ms / 877 | 0.50 ms / 714 | 0.06 ms / 133 |

**`AlphaMask::new` is 9.1% of a whole `forms_text_field` render and 15.6% of a
`forms_combo_box` one** — larger, on `combo_box`, than everything else in
`coverage_of` put together. `push_clip_mask`, which §11 took from 8.2 ms to
this, is now 0.12 ms; the band worked and the cost moved next door.

**`forms_number` shares it**, at 9.4% of its render on 40 pushes — which the
brief asked and which matters, because it says the cost scales with the clip
count on a document whose clip count is small.

### 12.3 Why it is a defect and not the price of a clip

`AlphaMask::new` is `vec![0; width * height]`. On this corpus's letter pages
that is **half a megabyte of `memset`**, and the allocator cannot hand it back
already zeroed once the page's first few planes are in flight — a fresh `mmap`
is zero-filled by the kernel, but a reused heap block is not. `forms_combo_box`
performs it two hundred and seventy-six times per render, for clips that are
about thirty rows tall: **140 MB of zeroing per render to describe about
sixteen megabytes' worth of clip.**

The plane must be device-sized — `Target` indexes it by absolute device row and
column, which §11.3 established and this does not change. What it need not be
is *newly allocated*, and the reason it was is that `coverage_of` had no other
source: a clip is pushed onto a stack and popped off it, and the popped one was
dropped.

### 12.4 The fix, and the invariant each half preserves

Both halves are inside `pdfrum-raster-agg`, and **neither changes the
arithmetic** — the same statement §11.4 makes, for the same reason.
`CFX_AggClipRgn::IntersectMask`'s truncating `a * b / 255` is untouched, and so
is the sweep.

**The pool.** `AggDevice` holds `planes: Vec<AlphaMask>`, and `coverage_of`
takes from it through `blank_plane` instead of calling `AlphaMask::new`. The
pool's invariant is that **every byte in it is zero**, which is exactly what
`AlphaMask::new` guarantees and exactly what the sweep needs: the sweep
*assigns* (`*slot = alpha`) rather than accumulates, so a recycled plane's spans
are the new path's; what the pool has to guarantee is only that the bytes the
sweep does **not** touch are zero.

**The band, again — this time to clear rather than to intersect.** `pop`
returns the popped plane to the pool through `recycle`, which restores the
invariant by clearing the *band* §11 already records for it. The band bounds the
plane's non-zero rows above: `coverage_of` wrote only inside its own band, and
`push_clip_mask`'s intersection only narrowed that, so clearing the band clears
everything that could be non-zero. Clearing thirty rows is what makes the reuse
worth having; clearing the whole plane would be the `memset` this exists to
avoid. The bands live in a `bands: Vec<Range<u32>>` parallel to `clips`, pushed
and popped with it.

**The reclaim takes the plane only when nothing else holds it.** `recycle` runs
*after* `sync_clip`, because the active target holds a share of the popped plane
until that repoints it one level out, and it goes through `Arc::try_unwrap`, so
a plane a layer still holds is simply not pooled. On this corpus that decline
never fires — **measured: zero declines over all forty-four `benches/corpus`
documents** — because `frames` is one LIFO and a layer opened after a clip is
always popped before it. It is kept regardless: `recycle` must not be the thing
that makes a future share unsound, and it costs one already-loaded refcount.

Three tests pin what the change trades on, and each targets one way it could be
wrong. `a_recycled_plane_carries_none_of_the_clip_it_held` pushes a clip over
rows 0..4, pops it, and pushes one over rows 4..8 — the second push takes the
first's plane, and rows 0..4 are rows the second sweep never touches, so an
uncleared byte there paints. `a_recycled_nested_plane_is_cleared_over_its_whole_band`
does the same where the recycled plane held an *intersection* narrower than its
recorded band, which is the case a band-clear that trusted the intersection
rather than the sweep would miss. `a_clip_under_a_layer_unwinds_with_the_layer_between_them`
is the stack shape the `try_unwrap` guards. Both of the first two fail under
either of the two mutations that matter — dropping the clear, and recording an
empty band.

**Nothing public moves.** `planes` and `bands` are private fields of
`AggDevice`, whose `clips` was already one; `AlphaMask::new` in `pdfrum-render`
is unchanged and still public. `scripts/api-snapshot.nu check` reports the
surface matching the committed baseline.

### 12.5 tinyskia: the clone §11.4 noted, and the larger one beside it

§11.4 recorded that `pdfrum-raster-tinyskia` "has a clone of the same shape in
its own `push_clip`/`push_clip_rect`… it pays one copy where AGG paid three
passes", and left it unmeasured. Looking at it found that description to be
right about the push and **wrong about where the crate's clone cost is**.

`tiny_skia::Mask::intersect_path` mutates in place, so a push genuinely must
clone the mask below it — that one is inherent and is left alone. What is not
inherent is that `fill_path`, `stroke_path` and `draw_image` each also cloned
the whole clip mask, **once per draw call**, and for a reason that is not about
tiny-skia at all: `self.target()` needs `&mut self` and `self.clip()` needs
`&self`, so the clip was cloned to end the borrow. `clips` and `layers`/`base`
are disjoint fields; a hand-split borrow (`target_and_clip`) returns both and
the clone goes away. `tiny_skia` takes the mask by reference either way, so
nothing about the drawing changes.

That is a device-sized `memcpy` **per drawn object under a clip** rather than
per clip push — on a forms page, hundreds where the push count is dozens.

### 12.6 The machine

**Load 43 rising to 131 on 32 cores**, sampled per fixture and reported in the
table's last column as a range rather than a point, because within a single
fixture's own measurement it moved by more than the fixture's own cost.

§10.4's tenants are still there — the two `python3` jobs are now at 407% and
393% CPU with elapsed times over two hours, and the dozen `osmium` processes
belong to the same other user — and they are joined by something §3 through §11
did not have to contend with: **three sibling agents on this repository, each
running its own conformance board and its own workspace build** in
`cargo-target/{tiling,m15-step2,writer-fonts}`. §3's instruction to re-take
these figures on an idle box is undischarged for the fifth time, and the reason
has changed shape: it is no longer only the other user's jobs.

**The consequence for this section is specific and is not hidden.** At load 76+
the interleaved A/B stopped resolving the effect: `forms_combo_box` read 0.98x
before/after in a window whose oracle column read 13.28 ms against §11's 4.38 —
the box three times slower, and the minimum of fifteen rounds dominated by
scheduling rather than by the code. The before/after column is load-independent
in the sense §3 argues *only while both arms see the same machine*, and at three
times the core count they do not reliably see it within one round.

**So the claim this section rests on is the profile, not the wall clock.**
§12.2's shares are taken inside one process, are a decomposition of that
process's own work, and do not change with what else the box is doing; §12.7
gives the same instrument's reading after the fix, which is the honest
before/after for a change whose whole content is "this function no longer costs
what it cost".

A wall-clock table is given anyway, in §12.8, with a shorter measurement unit
that did resolve the effect (forty five-iteration bursts rather than fifteen
twenty-one-iteration rounds) — and with two controls that push **zero** clips,
so their rows measure the instrument rather than the change. That pair is what
makes the table readable at this load: it says, in the same units as the claim,
how large the noise is.

### 12.7 The measurement that survives the load: the same instrument, after

Same probe, same documents, same warm loop, on the fixed binary:

| fixture | `AlphaMask::new`, before | `blank_plane` + `recycle`, after | removed |
|---|---:|---:|---:|
| `forms_text_field` | 0.605 ms (9.1%) | 0.104 + 0.033 = **0.137 ms** | **0.47 ms** |
| `forms_combo_box` | 1.551 ms (15.6%) | 0.076 + 0.062 = **0.138 ms** | **1.41 ms** |
| `forms_number` | 0.185 ms (9.4%) | 0.027 + 0.010 = **0.037 ms** | **0.15 ms** |

**The allocation is gone and the reclaim did not replace it**: `blank_plane` is
a `Vec::pop` and a size check, `recycle` is a refcount check and a band-sized
`fill(0)`, and together they are a fifth to a tenth of what they replace. The
pool reaches steady state after the deepest clip level the page ever holds —
which is why `recycle`'s call count equals `push_clip_rect`'s and the pool never
grows past the stack's high-water mark.

### 12.8 The wall-clock table, and the two controls that cannot move

Whole document, milliseconds, AGG, **lower is better**; the ratio is ours over
the oracle's, so **> 1 is pdfrum being slower**. `before` is `cd1a511`'s
parent, `after` is `cd1a511`. Read §12.6 before quoting any absolute
millisecond or any ratio: the machine tax is large here and the load column is
a *range* because it moved inside a fixture's own measurement.

*On the method:* §11.6's spelling — `n = 21`, best of five — could not resolve
the effect at this load (§12.6). What could is the same discipline with a
shorter unit and more of them: **forty alternating bursts of five iterations**,
which arm goes first swapped every burst, each arm's minimum kept. The argument
is `bench-oracle.nu`'s own: a timing sample is bounded below by the real cost
and unbounded above by the machine, so more independent short samples give a
better chance that one of them landed in an uncontended slice. The oracle
column is the same `(t[n] − t[1]) / (n − 1)` marginal-pass formula over ten
rounds, against PDFs copied to scratch because `pdfium_test` writes beside its
input and the oracle tree is read-only.

| fixture | clips/render | before (ms) | **after (ms)** | speedup | oracle (ms) | ratio before | **ratio after** | load |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `forms_combo_box` | 263 | 6.49 | **5.55** | **1.17x** | 3.91 | 1.66x | **1.42x** | 65–72 |
| `forms_list_box` | — | 11.10 | **10.29** | 1.08x | 5.87 | 1.89x | **1.75x** | 59–65 |
| `forms_number` | 39 | 1.78 | **1.69** | 1.05x | 1.33 | 1.34x | **1.27x** | 57–59 |
| `forms_push_button` | — | 5.77 | **5.36** | 1.08x | 73.69 | 0.08x | **0.07x** | 53–63 |
| `forms_signature` | — | 2.35 | **2.27** | 1.04x | 3.91 | 0.60x | **0.58x** | 56–58 |
| `forms_text_field` | 127 | 8.61 | **7.93** | 1.09x | 7.21 | 1.19x | **1.10x** | 56–65 |
| `forms_widgets_407` | — | 8.64 | **7.23** | **1.19x** | 9.72 | 0.89x | **0.74x** | 65–69 |
| **`forms` geomean** | | | | | | **0.80x** | **0.72x** | |
| | | | | | | | | |
| `vector_paths_1751` | **0** | 13.53 | 13.78 | 0.98x | 98.78 | 0.14x | 0.14x | 63–93 |
| `image_bug_583804` | **0** | 199.27 | 222.67 | 0.89x | 189.13 | 1.05x | 1.18x | 57–98 |
| `shading_axial_radial` | 33 | 32.33 | 36.38 | 0.89x | 61.94 | 0.52x | 0.59x | 65–83 |

**Every one of the seven forms rows improves**, from 1.04x to 1.19x, and the two
that improve most — `forms_combo_box` at 1.17x and `forms_widgets_407` at 1.19x
— are the two whose clip counts are highest. `forms_text_field`, the row §11.7
named, goes **1.19x → 1.10x** against the oracle. The class geomean goes
**0.80x → 0.72x**.

**The controls are the interesting half of this table, and they say something
§11.6's could not.** Two of the three push **zero clips per render** — measured,
by counting `push_clip_mask` calls in a warm render:

| fixture | clip pushes per render |
|---|---:|
| `forms_combo_box` | 263 |
| `forms_text_field` | 127 |
| `forms_number` | 39 |
| `shading_axial_radial` | 33 |
| `vector_paths_1751` | **0** |
| `image_bug_583804` | **0** |

**On `vector_paths_1751` and `image_bug_583804` the changed code does not
execute at all.** Their rows are therefore not a control in the usual sense —
that the fix did not hurt them — but a *calibration of the instrument*: they
read 0.98x and 0.89x, and every part of that spread is the machine. **A 0.89x
on a document the change cannot reach is the honest measure of what this box's
noise is worth**, and it is larger than three of the seven forms improvements.
Those three rows are real because the profile says so (§12.7) and because they
order with the clip count; they are not established by this table alone.

`shading_axial_radial` reads 0.89x on 33 clips per render, which by that
ordering should have shown a small gain and did not — the same noise, on a
document where the effect is smaller than it.

### 12.9 tinyskia, measured

The brief asked for this on the same three forms fixtures, before and after,
with `--backend tinyskia`. Both measurements are below and **they disagree**,
which is the useful part.

The wall clock, same forty-burst method, at load 72–86:

| fixture | before (ms) | after (ms) | speedup | load |
|---|---:|---:|---:|---:|
| `forms_combo_box` | 37.07 | **31.47** | **1.18x** | 72–76 |
| `forms_text_field` | 16.78 | 17.02 | 0.99x | 76–86 |
| `forms_number` | 8.75 | 14.59 | **0.60x** | 76–86 |

A 0.60x is not a thing this change can do — it removes a clone and adds
nothing — so the row is noise, and the in-process probe says which rows are and
which are not. Timing the clip access at `fill_path` and `draw_image`
directly, before and after:

| fixture | clone, before | access, after | removed | of the render |
|---|---:|---:|---:|---:|
| `forms_text_field` | 1.431 ms | 0.020 ms | **1.41 ms** | **6.0%** |
| `forms_combo_box` | 2.895 ms | 0.018 ms | **2.88 ms** | **7.0%** |
| `forms_number` | **0.004 ms** | 0.003 ms | 0.001 ms | **0.0%** |

**`forms_number` is the tinyskia analogue of §12.8's zero-clip controls.** Its
draws almost all run with **no clip in force**, and `self.clip().cloned()` on a
`None` clones nothing — so the clone this removes never cost that document
anything, and its 0.60x wall-clock row is measuring the machine, exactly as
`image_bug_583804`'s 0.89x is. `forms_text_field`'s 0.99x is the same, at a
smaller amplitude than the 6.0% the probe attributes.

So: the tinyskia change is worth **6–7% of a render on the two documents that
draw under a clip**, is worth nothing on the one that does not, and the wall
clock resolved only the largest of the three.

**Only the per-draw clone is removed.** `push_clip`/`push_clip_rect` still clone
the mask below them, because `tiny_skia::Mask::intersect_path` mutates in place
and the mask below must survive on the stack — that one is inherent, and AGG's
band trick does not port to it because `intersect_path` reports no band. §11.4
described *that* clone and did not see this one.

### 12.10 Conformance: nothing moved

**The board is byte-identical.** 1757 files, 1514 pass, 243 fail; every tag
bucket unchanged (`form-events` 8, `js-transcript` 33, `page-count` 2,
`pixel-fail` 41, `tierA-mismatch` 170); the text rates identical to six
figures. All **1757 per-file rows compare equal** to the committed
`conformance/scoreboard.json` — status, tags, the Tier A compared and
mismatched lists, and the Tier B `ssim`, `exact` and `max_channel_diff` on every
one. Not "no regressions": no row differs at all, which is what a change that
computes the same planes should produce.

**`conformance tier-c` is unchanged**: 1628 files compared under the gating
pair, 3 hard failures, 434 over the 1% edge budget, worst edge divergence
**66.6634%**, the same three named files. That is the cross-backend gate, and it
matters more here than in §11 because this section changes *both* rasterizers —
if the AGG pool or the tinyskia borrow had altered a pixel, the two backends
would have moved apart and this is the number that would say so.

`crates/pdfrum/tests/facade.rs`'s
`every_backend_renders_the_same_page_at_the_same_size` passes in the workspace
run, as do all 3926 tests.

### 12.11 What this does not claim

- **Not an idle box, for the fifth time**, and §12.6 says why the reason has
  changed: it is no longer only the other user's jobs. §3's instruction is still
  undischarged.
- **The wall-clock table is the weaker half of the evidence.** §12.8's own
  controls measure 0.89x on documents the change provably cannot reach, and
  three of the seven forms improvements are smaller than that. The claim rests
  on §12.7 and §12.9's in-process probes, which are decompositions of one
  process's own work; the wall clock corroborates it and does not establish it.
  A quiet re-take is owed and is on `docs/status/queue.md`.
- **The oracle column of §12.8 should not be compared with §11.6's.** It was
  taken at a different load on a different day; `forms_text_field`'s oracle
  reads 7.21 ms here against 2.59 ms there, which is the machine, not
  `pdfium_test`.
- **`vello_cpu` is unchanged.** It has a clip stack of its own that neither §11
  nor this section has looked at, and a figure taken on it still carries
  whatever it carries.
- **The pool is per device, not per session.** An `AggDevice` is built per page
  render, so the planes are freed with it; a document-lifetime pool would save
  the first push of each page and was not built, because the first push is one
  of two hundred and the `RenderCaches` seam it would have to cross is the
  engine's rather than this crate's.
- **The ratchet is still not re-baselined.** §8's third bullet stands, and
  `benches/baseline.json` is untouched.
