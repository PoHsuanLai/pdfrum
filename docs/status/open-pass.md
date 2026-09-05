# The open pass — the cross-reference table stops being a tree

**2026-09-06.** One cause, one commit: the merged cross-reference table was a
`BTreeMap<u32, XEntry>` and is now a slot vector indexed by object number.

`docs/benchmarks/losses-explained.md` had named the target precisely. Its
"warm open" row put our median at 0.14 ms against hayro's 0.04, split the
cost — 77.5% the cross-reference chain, 14.2% the eager page-tree walk — and
then divided that first number again into a part worth keeping and a part
that was not:

> The **52% of open spent in `BTreeMap` insert plus `merge_up`** is not a
> tradeoff … That is the fixable half and it is the larger one.

This pass is that half. The 14.2% page walk is untouched and stays: it buys
an infallible `page_count()` and the damaged-file recovery the benchmarks
credit us with.

## Method

Callgrind `Ir`, `benches/src/bin/profile`, one iteration, four files —
`text_quick_start.pdf`, `mixed_formfield.pdf`, `forms_widgets_407.pdf`, and
`vector_font_feature.pdf`. `forms_widgets_407` is the corpus's largest
cross-reference at 9950 objects, which is where a per-entry cost shows
itself; `vector_font_feature` (1475) is the largest of the rest.

`Ir` rather than wall-clock deliberately. This machine ran at a load average
of 20 to 40 on 32 cores throughout the pass, and the comparison harness
could not resolve a change of this size through that contention — see
"The harness" below.

## The cause, confirmed before changing anything

Baseline on `text_quick_start.pdf`: 5 350 120 `Ir` whole process,
`Document::from_bytes` 4 546 021 of it (84.97%). Inside that:

| item | `Ir` | share of open |
|---|---|---|
| `BTreeMap::insert` | 1 271 428 | 28.0% |
| `Xref::merge_up` | 1 087 558 | 23.9% |
| **the container, together** | **2 358 986** | **51.9%** |

The document's 52% reproduced exactly.

Both numbers are per *entry*. `add_compressed` inserted one map entry per
compressed object, and `merge_up` re-inserted every key of every section
while walking `/Prev` — a tree descent and a node allocation each time, for
a load whose actual work is writing each object's entry once per section
that names it.

## The change

Object numbers are dense. A trailer `/Size` is a claim that the file's
objects are numbered `0..size`, and real files honor it, so the table is a
near-complete range rather than a sparse scattering. `EntryTable`
(`crates/pdfrum-parser/src/xref/mod.rs`) is therefore
`Vec<Option<XEntry>>` indexed by object number, with an incrementally
maintained occupied count. Reading or writing a slot is an offset
computation; `merge_up` is one linear pass over the occupied slots.

What did **not** change is every rule about which entry wins: the generation
comparison in `add_normal`, the generation-and-container refusal in
`add_compressed`, the sticky object-stream flag, the `/Size` truncation, and
the asymmetric trailer merge. The rules are applied in the same order on the
same values — only the container beneath them is different. No new
dependency; the public API is unchanged, which `scripts/api-snapshot.nu
check` confirms.

## Open — `Ir`, `--op open --iterations 1`

| file | before | after | change |
|---|---|---|---|
| `text_quick_start.pdf` | 5 350 120 | 2 713 090 | **-49.3%** |
| `mixed_formfield.pdf` | 3 073 204 | 2 842 988 | -7.5% |
| `forms_widgets_407.pdf` | 14 561 868 | 4 791 409 | **-67.1%** |
| `vector_font_feature.pdf` | 2 822 594 | 1 786 634 | **-36.7%** |

The gain tracks the number of entries, as a per-entry cost should:
`forms_widgets_407` with 9950 objects gains most, `mixed_formfield` — whose
open is dominated by other work — least.

Inside `text_quick_start`'s open, the container has essentially left the
profile:

| item | before | after |
|---|---|---|
| `BTreeMap::insert` | 1 271 428 | — (gone) |
| `Xref::merge_up` | 1 087 558 | 127 989 |
| `Xref::add_compressed` | 1 398 475 | 110 450 |

`Document::from_bytes` falls from 84.97% of the process to 73.34%, and the
largest single item inside it is now `catalog_page_count` at 23.6% — the
eager page-tree walk, which is the part we are keeping on purpose.

## Lookups did not regress — `Ir`, render and text

The table is read once per `resolve`, so a change to it has to be checked in
the other direction too. An array index beats a tree descent, so these
improved rather than held:

| file | render before | render after | text before | text after |
|---|---|---|---|---|
| `text_quick_start.pdf` | 2 785 325 195 | 2 783 008 348 | 137 672 915 | 135 531 786 |
| `mixed_formfield.pdf` | 639 917 220 | 639 523 297 | 3 207 657 | 2 980 601 |
| `forms_widgets_407.pdf` | 217 155 950 | 206 793 148 | 24 844 175 | 15 267 833 |
| `vector_font_feature.pdf` | 1 179 512 864 | 1 178 235 432 | 264 948 859 | 263 677 589 |

`forms_widgets_407` again shows the largest move (-4.8% render, -38.5%
text), for the same reason: it has the most entries to look up.

## The board — the proof the merge order survived

`conformance/scoreboard.json`, 8 jobs:

```
run: 1759 files, 1547 pass, 212 fail
  form-events 8, js-transcript 7, page-count 2, pixel-fail 34, tierA-mismatch 170
  text 1785/2067 pages (86.4%), text-nonempty 760/1005 (75.6%)
no regressions against conformance/scoreboard.json
```

Compared row by row against `board-pathclose-main.json`: **0 changed rows**
of 1759, totals byte-identical, no row added or removed. Of those, **770 are
the `FRC` family and the `bug_*` files with broken cross-references** — the
damaged-file recovery set — and none moved. That set is exactly where a
merge-order mistake would surface, which is what makes it the gate rather
than the pass rate.

## The harness

`benches/compare`, `--label open --corpus ../corpus --engines pdfrum`, run
alternately before/after twice to share load conditions:

| round | binary | load (1 min) | cold median | warm median | warm p95 |
|---|---|---|---|---|---|
| 1 | before | 20.30 | 0.63 | 0.24 | 5.15 |
| 1 | after | 27.05 | 0.45 | 0.25 | 4.88 |
| 2 | before | 20.33 | 0.40 | 0.20 | 4.54 |
| 2 | after | 19.44 | 0.70 | 0.19 | 10.66 |

**This table measures the machine, not the change.** At a load average of 20
to 27 on 32 cores the warm medians swing more between two rounds of the
*same* binary (0.24 to 0.20 for "before") than between the two binaries, and
an improvement of this size sits below that noise floor. The callgrind
counts above are the measurement that holds; this one is recorded so the
next person does not repeat it on a busy machine and conclude nothing
happened. Re-run it on an idle box to put a wall-clock number on the row.

## Dead-code sweep

Nothing to remove. The pass replaced a container in place: every
`EntryTable` method has a caller, no helper, field, variant or constant was
orphaned, and no test lost its only subject. The dead-code lint runs as part
of `-D warnings` and fires on private items, which it did not. The only
remaining mentions of `BTreeMap` in the crate are the two doc comments that
explain what the type used to be and why it stopped being that.

## Gates

`cargo fmt --all --check`; `cargo clippy --workspace --all-targets` with the
javascript features, `-D warnings`; `cargo nextest run` over
`pdfrum-parser`, `pdfrum-object`, `pdfrum`, `pdfrum-cli`, `pdfrum-edit` —
1027 passed; `cargo test --doc -p pdfrum-parser` — 9 passed;
`RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace`;
`nu scripts/api-snapshot.nu check` — green, the public API is unchanged;
the wasm32 facade check; and the fuzz workspace `cargo check --all-targets`.
All green.

Clippy is worth a note: `indexing_slicing` and `cast_possible_truncation`
are denied in this tree, so `EntryTable` reaches its slots through
`get`/`get_mut` and converts indices with `u32::try_from` rather than `as`.
That is not a concession — a slot vector is exactly the place where a
silent out-of-range index would be a security bug, and the lints keep the
bounds reasoning explicit.

## What is left in the open row

With the container gone, open on `text_quick_start` is 2 713 090 `Ir` and
splits roughly: `read_stream_section` 28.4% (inflating the cross-reference
stream — real decompression work), `catalog_page_count` 23.6% (the eager
page walk we are keeping), and the remainder spread across parsing. There
is no longer a single dominant fixable item; the next open win would have to
come from the stream decode or from making the page walk lazy, and the
latter is the API property `losses-explained.md` argues we should keep.
