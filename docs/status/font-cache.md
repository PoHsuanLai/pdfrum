# Per-document font cache — M21 item 3

**Method:** M18's (`docs/status/M18.md`) — callgrind `Ir`,
`callgrind_annotate --inclusive=yes`, a named cause per commit with the
before/after on the same three runs, and the conformance board compared row
by row. The box is shared and loaded, so `Ir` is the number that decides and
wall-clock throughput is quoted only with the load average it was taken at.

**The queue row this answers** (`docs/status/queue.md` M21 item 3): parallel
render scaling flattens after 4 threads — 39 → 71 → 75 pages/s against
hayro's 70 → 156 → 167 and mupdf's 149 → 220 → 229 — "because each new
`RenderSession` enumerates fonts".

## 1. The profile, before anything was changed

The row's stated cause is **wrong**, and measuring before fixing is what
caught it. Three things were separated: per-page font reload, per-session
database enumeration, and glyph cache misses.

### 1.1 Per-page reload: already solved, on the single-session runs

`docs/status/text-perf.md` §6 named `pdfrum-font::load_with_options` at 29%
of `text_tcpdf_063` and read it as "a `/ToUnicode` CMap parsed per font per
page build". It is 32.9% (100.9 M of 307.0 M), and it is **not** per page.
Callgrind's own call counts:

| run | pages | distinct `/Font` refs | calls to `load_with_options` |
|---|---:|---:|---:|
| `--op text text_tcpdf_063 --iterations 1` | 11 | 4 | **4** |
| `--op render text_quick_start --iterations 1` | 11 | 10 | **10** |

One load per distinct font, not one per page. `BuildContext.font_instances`
— a memo keyed on the resource's `ObjRef`, living on the session — was
already doing its job perfectly for a run that holds one session.

So the 100.9 M is four fonts loaded **once each**, and `tounicode::parse` is
75.9 M of it. `text_tcpdf_063`'s object 30 is an identity `/ToUnicode`: 256
`bfrange` entries each spanning 256 codes, 65 536 mappings expanded
one-by-one into a `BTreeMap`, ~19 M `Ir` per parse. That is a **storage**
problem in `ToUnicode` (a contiguous range stored as a range rather than
expanded), not a caching one, and it is recorded in §6 as not fixed.

### 1.2 Per-session database enumeration: does not happen

`subst::resolve_with_options` (`crates/pdfrum-font/src/subst/mod.rs:232`)
already caches the host scan in a process-wide `OnceLock`, with a comment
recording the measurement that justified it. `SystemFontDb::scan` does not
appear in any of the profiles below, at any thread count, at any threshold.

**Step 3 of this pass was therefore skipped.** There is no per-session font
database to share; the queue row's stated mechanism is not real, and adding
a cache the profile does not justify would have been the wrong change.

### 1.3 Per-session *loading*: this is the real cost

What actually grows with the thread count is the loading, because the memo
that made §1.1 free lives on the **session**, and the throughput harness
gives each of N threads its own. Callgrind over the whole process,
`--separate-threads=no`, on a 3-file corpus (`text_tcpdf_063`,
`text_quick_start`, `text_foxit_products`):

| | 1 thread | 8 threads | growth |
|---|---:|---:|---:|
| whole process | 24,657,718,071 | 26,734,604,900 | +2,076,886,829 |
| `load_with_options` | 262,908,876 | 1,407,154,053 | **+1,144,245,177** |
| — `tounicode::parse` | 153,952,822 | 768,952,131 | +615 M |

**55% of all the extra work at 8 threads is redundant font loading**, and
`load_with_options` grows 5.35× for the same pages. That is the flattening,
and that is what this pass fixes.

## 2. The cache (`04c4779`)

`pdfrum_font::FontCache` already documented itself as "per-document caches:
parsed faces, resolved substitutions, font identities" while holding only a
`next_id: AtomicU64`. It now holds what it claimed:

- `get_or_load(reference, load)`, keyed on the **`ObjRef` newtype** that
  named the `/Font` resource, valued `Arc<Font>` — so a hit shares the whole
  loaded font: its parsed `/ToUnicode`, its CID tables, its glyph cache.
- An **inline** font dictionary has no reference and is not cached. Two
  inline copies genuinely are two fonts and there is no document-scoped
  identity to key them on.
- `None` is cached too: a dictionary that will not load is as stable an
  answer as one that will.
- `RwLock<HashMap<..>>`, no new dependency. The loader runs *outside* the
  lock, so two threads wanting two different fonts do not serialize; two
  racing on the same reference may both load, and whoever inserts first is
  the shared instance for everyone. That costs one duplicate parse and keeps
  the loader off the lock.

`BuildContext.fonts` becomes `Arc<FontCache>`; the facade's `Document` owns
one and `Document::render_session()` hands it to every session, so a text
run and a render run over one document share it as well as N workers do.
`BuildContext.font_instances` is subsumed and deleted.

The `/ExtGState` `/Font` path went through **neither** memo before. Its two
by-reference spellings now key on the same identity `Tf` does, so a font
named both ways loads once.

### Substitution, and why it is not part of the key

`BuildContext`'s own docs already require that every load under one document
make the same substitution choice — "a substitution that varied between two
`Tf` operators naming the same resource would give one line of text
different metrics from the next". A cache is created for one set of options
and used with those; the caller that owns the options owns the cache.

## 3. Ir, per commit

`benches/src/bin/profile`, callgrind, `--inclusive=yes`:

| run | before | after | |
|---|---:|---:|---:|
| `--op text text_tcpdf_063 --iterations 1` | 306,989,043 | 307,283,846 | +0.1% |
| `--op render text_quick_start --iterations 1` | 4,014,017,833 | 4,013,993,884 | −0.0% |
| throughput, 3 files, 8 threads, whole process | 26,734,604,900 | 25,502,204,070 | **−4.6%** |
| — of which `load_with_options` | 1,407,154,053 | 350,547,583 | **−75.1%** |

**The two single-session runs are unchanged on purpose.** §1.1 is why: their
memo was already perfect, so there was nothing to win, and the +0.1% is the
`Arc` clone and the `RwLock` read. Quoting a win there would have meant
measuring something else. Font loading at 8 threads is now 1.33× its
1-thread cost rather than 5.35×; the residue is the race window §2 documents.

## 4. Throughput

`benches/compare`, `benches/corpus`, 18 files with ≥4 pages, 156 pages,
`--engines pdfrum --threads 1,4,8`. Both runs taken **back to back** at the
same load, because an earlier "before" run at load ~32 and an "after" at
load ~11 are not comparable and the first pair was discarded for that reason.

| threads | before | after | |
|---|---:|---:|---:|
| 1 | 48.3 | 47.1 | −2.5% |
| 4 | 82.9 | **93.5** | **+12.8%** |
| 8 | 87.3 | **98.1** | **+12.4%** |

Load average: 10.14 → 9.46 across the before run, 11.81 → 9.68 across the
after run. 32 cores, shared box, one other user. The 1-thread row is within
this box's noise and the `Ir` for it is flat, so it is read as unchanged
rather than as a regression.

The harness change is part of this: `benches/compare/src/engines/pdfrum.rs`
builds its session with `doc.render_session()` in both branches, which is
what a caller has to do to get the sharing. `RenderSession::new()` still
gives an unshared cache, which is the right answer for a caller with no
document to hang one on.

## 5. Board

`conformance run`, the same tree with and without the change, `per_file`
compared row by row:

```
before rows 1759  after rows 1759
changed 0  added 0  removed 0
```

1759 files, 1537 pass, 222 fail on both sides, every bucket identical
(form-events 8, js-transcript 8, missing-golden 4, page-count 2, pixel-fail
41, tierA-mismatch 170; text 1781/2063 pages, text-nonempty 758/1003). A
font cache must not change a pixel or a character, and it does not.

## 6. Dead-code sweep

The pass left exactly one item unreachable and it went out in the change
that replaced its use, so there is no separate deletion to make:

- **`BuildContext::font_instances`** (`crates/pdfrum-page/src/build.rs`) —
  the per-context `HashMap<ObjRef, Option<Arc<Font>>>` memo. Subsumed by
  `FontCache`'s cache, which is the same map with a longer life. Deleted.

Everything else the pass touched was checked against its callers and kept:
`BuildContext::with_substitution` (9 callers), `FontCache::new` (27),
`RenderSession::new`/`default` (43), `Resources::find_ref` (4). A
`--all-features` build with `-W dead_code -W unused` reports nothing.

## 7. What was found and not fixed

- **`tounicode::parse` expands contiguous ranges.** §1.1: an identity
  `/ToUnicode` costs ~19 M `Ir` because 256 `bfrange` entries spanning 256
  codes each become 65 536 `BTreeMap` entries. The cache makes a document
  pay it once instead of once per session, but a document with one such font
  still pays 19 M for it. Storing a `Consecutive` range **as a range**, with
  `lookup` doing the arithmetic, would make it near-free — and `handle_bfrange`
  already builds exactly that `Range::Consecutive` value before flattening it
  in `commit_range`. That is a `ToUnicode` storage change with its own
  reverse-map and collision-ordering consequences, so it is a named item for
  a later pass rather than something to fold into a caching one.
- **Diagnostics are first-loader-wins.** A font loaded first through the
  `/ExtGState` path records into `Diagnostics::with_limit(0)` and a later
  `Tf` for the same object sees none. This was already true within a session
  under `font_instances`; the cache extends it across sessions. The board is
  byte-identical, so nothing observable depends on it today.
- **The 1-thread throughput row.** −2.5% at load ~10, flat in `Ir`. If a
  quiet box shows it repeatably it is the `RwLock` read on the hot path and
  the answer is a per-session front cache in front of the shared one — but
  it is not distinguishable from noise here and should not be chased on this
  box's numbers.

## 8. Gates

Per commit, all green:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features pdfrum-tool/javascript,pdfrum-cli/javascript -- -D warnings
cargo nextest run -p pdfrum-font -p pdfrum-page -p pdfrum -p pdfrum-text \
  -p pdfrum-render -p pdfrum-cli        # 1787 passed, 2 skipped
cargo test --doc -p pdfrum -p pdfrum-font                # 102 passed
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace
nu scripts/api-snapshot.nu check                         # green
cargo check -p pdfrum --target wasm32-unknown-unknown --no-default-features \
  --features vello-cpu,codecs-all,forms,edit,markdown
```

1787 is 1784 plus three new ones pinning the cache: one reference loads once
and both asks get the same `Arc`; a second reference loads separately and a
failure is cached; eight threads sharing one cache all come away with a font
and a ninth ask afterwards reaches no loader.

`docs/status/api-baseline` regenerated — three additions, no removals:
`Document::render_session`, `FontCache::get_or_load`, and `BuildContext`'s
`fonts` field becoming `Arc<FontCache>`. The wasm32 check confirms the cache
pulls in nothing std-only beyond what was already there.
