# Text extraction speed — M21 item 2

**Method:** callgrind `Ir` on the `profile` binary (`perf` is closed on this
box), `--op text --iterations 1`, whole process. Conformance board
byte-identical after every landing: 1759 files, 1537 pass, `per_file` rows
compared row by row against `board-m20c.json` — 0 changed, 0 added, 0
removed on each of the three commits. Gates per commit: `cargo fmt --all
--check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo
nextest run -p pdfrum-font -p pdfrum-text -p pdfrum -p pdfrum-cli -p
pdfrum-markdown` (990 tests), `cargo test --doc`, `RUSTDOCFLAGS='-D
warnings' cargo doc --no-deps`.

**Starting point** (`docs/status/M21.md`): warm text extraction of page 1,
0.58 ms median on `benches/corpus/`, against pdfium-render's 0.05 ms —
ten times PDFium, while correctness leads.

## 1. Where the time was: not the page build

The queue's guess was that "the text page is built eagerly with boxes for
every character". The profile says the eager boxes are real but the cause
sits one layer down, in the font accessors they call. Inclusive `Ir` at the
start, `Page::text_on` as 100%:

| | `text_quick_start` | `text_foxit_products` | `text_tcpdf_063` |
|---|---:|---:|---:|
| whole process | 147.8 M | 328.2 M | 2,890.5 M |
| `pdfrum_page` build | 44.8% | 9.9% | 18.2% |
| **`pdfrum_text::extract`** | **26.7%** | **75.6%** | **80.4%** |

The page build dominates only on the *smallest* file. On the two text-heavy
ones the text crate is three quarters of the run, so this was the text
crate's item to take, and the page build is not the answer.

Inside `extract`, three causes, in the order the profile named them.

## 2. The page-flow guess built every text object a second time (`0eb0a48`)

`orientation::page_flow` called `object::build` on every page-level text
object purely to read its `rect` — and `extract` had already built every one
of them a line earlier (`object::walk` at `lib.rs:138`, then `page_flow` at
`:139`). Every glyph box, char width and pen walk on the page was done
twice. `page_flow` was 28.5% of `text_tcpdf_063` and 21.6% of
`text_foxit_products`.

`page_flow` now takes the built runs. `object::top_level_text_indices`
reproduces `collect`'s flattened numbering without building anything, so the
two ascending sequences pair in one forward scan and the guess still counts
only the objects the page itself lists — text inside a form `XObject` stays
uncounted, as `FindTextlineFlowOrientation` requires.

| `Ir` | before | after | |
|---|---:|---:|---:|
| `text_tcpdf_063` | 2,890.5 M | 2,064.4 M | **−28.6%** |
| `text_foxit_products` | 328.2 M | 258.5 M | **−21.3%** |
| `text_quick_start` | 147.8 M | 141.6 M | −4.2% |

## 3. A bare CFF drew every glyph to read its advance (`dd502f0`)

With the double build gone, one function was **84% of the whole
`text_tcpdf_063` run**: `GlyphSource::advance_tt` → `Face::advance`, 1,740.6 M
of 2,064.4 M. A bare CFF carries no `hmtx`, so the advance lives inside the
charstring, and `Face::advance` read it by running the Type 2 interpreter
over the glyph's entire outline into a `PathPen` and throwing the path away
(`CffFontRef::draw` 822.8 M, `read_fonts::ps::cs::evaluate` 796.4 M).
Nothing remembered the answer, and extraction asks for a width once per
shown character — `Font::char_width`, then again through the extractor's own
`ladder_char_width` — so the same handful of glyphs were drawn hundreds of
times each.

`Face` now carries an `advances` cache beside the `hinting` and `names`
locks it already keeps. This is a font-side accessor, so it is the one
change outside `pdfrum-text`, and it is the cause rather than a workaround.

| `Ir` | before | after | |
|---|---:|---:|---:|
| `text_tcpdf_063` | 2,064.4 M | 344.3 M | **−83.3%** |
| `text_foxit_products` | 258.5 M | 257.4 M | −0.4% |
| `text_quick_start` | 141.6 M | 141.6 M | 0 |

The last two are SFNT faces with an `hmtx`, whose advance never took the
draw path — the win is exactly the bare-CFF font the profile named.

## 4. Every glyph box rebuilt the whole metrics table (`172fbb2`)

The same shape, now the top item on `text_foxit_products`: `Face::glyph_bbox`
at 28.5% (73.2 M of 257.4 M), of which `skrifa::GlyphMetrics::new` was
47.0 M. It rebuilt a `skrifa::FontRef` and a whole `GlyphMetrics` — `hmtx`,
`loca`, `glyf`, the variation tables — to read one glyph's box, once per
shown character, asked for through `TextRun::glyph_bbox` and again by the
width ladder's last rung.

A `boxes` cache beside `advances`. Both caches key on the `Gid` newtype
rather than a bare `u16`: zero-cost (`Ir` identical, 194,248,589 →
194,257,192 on `text_foxit_products`), and it makes a glyph id in the wrong
index space — `pdfrum_type1::Gid` is a separate type for exactly that
reason — a compile error rather than a silent miss.

| `Ir` | before | after | |
|---|---:|---:|---:|
| `text_foxit_products` | 257.4 M | 194.3 M | **−24.5%** |
| `text_tcpdf_063` | 344.3 M | 307.6 M | **−10.6%** |
| `text_quick_start` | 141.6 M | 137.7 M | −2.8% |

## 5. Where it landed

Whole process `Ir`, start of the pass to end:

| file | before | after | |
|---|---:|---:|---:|
| `text_tcpdf_063` | 2,890.5 M | 307.6 M | **−89.4%** |
| `text_foxit_products` | 328.2 M | 194.3 M | **−40.8%** |
| `text_quick_start` | 147.8 M | 137.7 M | −6.8% |

`benches/compare/`, `benches/corpus` (44 files), both engines in the same
run on the same box:

| engine | op | cold median | **warm median** | warm p95 |
|---|---|---:|---:|---:|
| pdfrum, M21 | text | — | **0.58 ms** | — |
| **pdfrum, now** | text | 5.50 ms | **0.39 ms** | 2.45 ms |
| pdfium-render | text | 0.11 ms | 0.05 ms | 0.58 ms |

**0.58 → 0.39 ms, −33%.** The gap to PDFium narrows from 11.6× to 7.8×.
Correctness is unmoved: text extracted 44/44, whitespace-normalized 39/44
(89%), token F1 ≥ 0.9 on 43/44, median token F1 1.000, zero errors, panics,
crashes or timeouts — and "Losses — files where a peer is closer to the
oracle than pdfrum: None".

## 6. What is left, and where it lives

The text crate's own profile is now flat: after the three changes no
`pdfrum-text` function exceeds 2% self-cost on `text_foxit_products`, and
the largest single remaining item is the allocator (`_int_malloc` +
`_int_free` + `malloc` ≈ 13% combined), diffuse across the pipeline rather
than attributable to one call. A further pass here would be a broad
allocation rework, not an M18-style named cause.

The two costs that now lead, both **outside this pass's remit**:

- **`pdfrum-page` build, 39.3% of `text_tcpdf_063`**
  (`crates/pdfrum-page/src/build.rs`, `interpret_streams`, 135.1 M of
  307.6 M). Text extraction builds the full page graph, including work no
  text consumer reads. `docs/status/M13-perf-baseline.md` §23.4 already
  records the extreme case — on `image_bug_583804`, 87% of the text run is
  `image::unpack`, because there is no build mode that skips decoding. That
  is the next item for whoever owns `pdfrum-page`, and it is worth more than
  anything left in the text crate.
- **`pdfrum-font` load, 29.3% of `text_tcpdf_063`**
  (`crates/pdfrum-font/src/lib.rs`, `load_with_options`, 100.9 M), of which
  `tounicode::parse` is 75.9 M — a `/ToUnicode` CMap parsed per font per
  page build, with `cid::load` a further 79.1 M. Per-document font caching
  is the answer, and it is the same shared-font-database item M21 item 3
  already names for parallel scaling.

  **Corrected 2026-09-05** (`docs/status/font-cache.md` §1.1): "per font per
  page build" is wrong. Callgrind's call counts say `load_with_options` runs
  **4 times** on this file — once per distinct `/Font` reference, over 11
  pages — because `BuildContext.font_instances` already memoized it. The
  100.9 M is four fonts loaded once each, and the 75.9 M is one identity
  `/ToUnicode` (256 `bfrange` entries spanning 256 codes each, 65 536
  mappings expanded into a `BTreeMap`) costing ~19 M `Ir` per parse. Caching
  moved that cost off the per-session path, where it was real and now is
  not; making the parse itself cheap is a `ToUnicode` storage change and is
  still open.

## 7. The ratchet

`cargo bench -p pdfrum-text` into
`CARGO_TARGET_DIR=/mnt/data2/r13921098/cargo-target/main-bench`, then
`cargo run --release -p pdfrum-bench --bin ratchet -- check`. The full
`--workspace` bench does not fit this session's foreground command cap, so
the text crate's groups — the ones this pass moves — were re-measured and
the rest of the 440 rows are the target dir's existing measurements.

```
ratchet: 440 benchmarks measured, 440 in the baseline
  166 unchanged, 163 improved, 111 regressed, 0 new, 0 not run
```

The pass shows up directly in the text rows:

| row | before | after | |
|---|---:|---:|---:|
| `text/text_text_tcpdf_063` | 138.825 ms | 17.600 ms | **−87.3%** |
| `text/mixed_mixed_tcpdf_059` | 13.434 ms | 1.532 ms | **−88.6%** |
| `text/text_text_tcpdf_055` | 64.320 ms | 13.219 ms | **−79.4%** |
| `text/shading_shading_tcpdf_030` | 2.525 ms | 222.979 us | −91.2% |
| `text/forms_forms_number` | 1.285 ms | 100.509 us | −92.2% |

The 111 flagged regressions are render/save rows in a +3–4.5% band that this
pass does not touch and that were **not re-measured** in this run. They are
the machine-state effect `docs/status/M13-perf-baseline.md` §23 documents at
length — this box is loaded — not code. `ratchet update` was **not** run:
the baseline should be re-taken from a full `--workspace` bench on an idle
box, and doing it from a partial run would write those stale rows in.

## 8. Dead-code sweep (`2920ed3`)

Per the standing rule, the pass ends with a sweep of what it touched.
`TextRun::original_rect` deleted — a `pub` field with no reader in the
workspace, written and consumed one line later inside `layout`, now a local
(this removes a public field, so `docs/status/api-baseline/pdfrum-text.txt`
is regenerated alongside; the diff is that one line). `object::build`
narrowed `pub` → `pub(crate)`, since `orientation` was its only caller
outside the module and §2 removed that call. Everything else in the touched
files was checked against its callers and kept.
