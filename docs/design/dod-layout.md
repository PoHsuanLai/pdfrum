# Data layout pass — what the profile says a DOD pass should touch

**Status:** Scoped 2026-09-04; PLAN.md M18. Not started.
**Scope:** `pdfrum-object`, `pdfrum-page` (lexer, state), `pdfrum-font` (glyph names).
**Method:** callgrind instruction counts (`Ir`) on the `profile` binary built from
`main` at `5142672`, cold render (`--op render`, fresh session per iteration,
`agg`), two iterations. `perf` is closed on this machine
(`perf_event_paranoid = 4`, no sudo — the same state M12b-P2 recorded), and
`Ir` has the property this box needs: it does not move with load, which stood
at 8 during every run below. Wall times are the binary's own per-iteration
figure at that load and are indicative only.

## 1. The proposal, and what was checked

The proposal named four data-layout changes: box `Object::Stream`, share or
index the per-object `GraphicsState` and de-box `PageObject`, keep a
contiguous bounding-box array for the cull, and replace kurbo's `BezPath` with
verbs+points. Each carried a premise about where time goes. The premises were
checked before anything was scheduled, because M12b showed twice that a
layout argument made from type sizes alone points at the wrong loop (P2:
the arena, +265% slower; P3: the cull, expensive because it was *exact*, not
because it ran often).

### 1.1 Sizes, measured (`size_of`, x86-64)

| Type | Proposal said | Measured | Note |
|---|---:|---:|---|
| `Object` | 64 | **56** | largest variant `Stream` = `Dict` 24 + `ByteSpan` 32 |
| `Name` | 32 | **24** | `Cow<'static, [u8]>` |
| `PdfString`, `Dict`, `Array` | — | 24 each | |
| `(Name, Object)` dict pair | 96 | **80** | |
| `Object` with `Stream(Box<Stream>)` | 32 | **32** | all remaining payloads are 24 + tag |
| dict pair after boxing | 64 | **56** | |
| `PageObject` | — | **16** | tag + `Box` |
| `Content<PathObject>` | — | 472 | `GraphicsState` 344 + `PathObject` 80 + marks 24 + rest |
| `GraphicsState` | ~350 | **344** | `ClipStack` 32 (a `Vec<ClipEntry>` — the only deep part), `TextState` 72, `ColorValue` 48 ×2, `StrokeParams` 48, `GeneralState` 48, `Affine` 48 |
| `kurbo::PathEl` | 56/64 | **56** | |

The size arithmetic in the proposal is right in direction and slightly high
in every figure. The question is which of those bytes a hot loop touches.

### 1.2 Where a cold render spends its instructions

`vector_en_tem` (the document the proposal's "94% page build" figure came
from; 62.9 ms/iteration cold at load 8):

| Inclusive `Ir` | Where |
|---:|---|
| 77.7% | `Page::prepare` (the build) |
| 74.7% | └ `pdfrum_font::load_with_options` — **font loading** |
| 58.2% | &nbsp;&nbsp;└ `Face::name_index` — one linear scan of every glyph per looked-up name |
| 29.4% | &nbsp;&nbsp;&nbsp;&nbsp;└ `read_fonts::Post::glyph_name`, whose `string_data().get(idx)` walks the Pascal-string array from the start |
| 21.8% | `PreparedPage::render_on` (the walk + the rasterizer) |
| 1.7% | `GraphicsState` clone/drop, all sites |
| 2.8% / 3.2% | `memcpy` / `calloc`, whole process |

`vector_paths_1751` (5 010 paths, the M12b reference document; 8.8 ms/iteration cold):

| Inclusive `Ir` | Where |
|---:|---|
| 67.7% | `Page::prepare` |
| 46.8% | └ `content::parse_content` — **the content lexer** |
| 38.0% | &nbsp;&nbsp;└ `ContentLexer::next_element` |
| 22.3% | &nbsp;&nbsp;&nbsp;&nbsp;└ `word_to_number` (`str::from_utf8` + `str::parse::<f32>` / `dec2flt`) |
| 12.5% | └ `interpret_streams` (the state machine, including every `content()` clone) |
| 7.3% | └ flate |
| 30.3% | `PreparedPage::render_on` |
| 0.9% | └ `culled` |
| ~12% | `malloc`/`free`/`realloc` self time, whole process — `next_word` returns `word.to_vec()`, one heap allocation **per token** (`tokenize.rs:325-374`) |

### 1.3 Verdicts

- **A. `Object::Stream(Box<Stream>)` — do it, as a memory change.**
  56 → 32 bytes per `Object`, 80 → 56 per dict pair. ISO 32000-1 §7.3.8.1 makes
  a direct stream in an array or dict illegal, so the box is dereferenced only
  on the parser's indirect-object path. No hot loop above is `Object`-bound
  (`open/*` is microseconds), so the expected win is resident memory and
  parse traffic, not a render number. Exit is "no ratchet regression, `open/*`
  and `build/*` not slower, RSS measured on the ten largest corpus files". It
  changes a public type in `pdfrum-object`; `api-snapshot update` is
  deliberate, and the M13 release has not shipped it yet.
- **B. Shared `GraphicsState` / de-boxed `PageObject` — declined on the
  numbers.** The state clone is 1.7% of the document the proposal cited and
  invisible on the other. The "94%" was `render-cold` on a document whose
  build is font loading, and the font loading is one quadratic lookup
  (§1.2). De-boxing `PageObject` is a public-API change (`pdfrum::PageObject`
  is the editor's type) with no loop to pay for it. One narrower question
  survives and is **kept as a measurement**: `ClipStack` is the one deep part
  of the clone (each entry owns a `BezPath`), and no corpus document was
  profiled with a deep clip stack. M18 counts clip entries cloned per page
  across the 44-document corpus with the existing `renderprofile` counters;
  if any document spends ≥ 3% of its build there, the entries become
  `Arc<ClipEntry>` (a one-line change in `state/clip.rs`, no public type
  moves). Otherwise the number is written down and the item closes.
- **C. Contiguous bounding boxes for the cull — declined.** M12b-P3 §5.1
  already took the cull from 156 ns to ~48 ns per object (−69%) by bracketing
  the exact box; it is now 0.9% of a cold render and 2.1% of the engine half.
  A SoA sweep cannot recover more than that. Reopen only if a walk profile
  shows it above 5%.
- **D. Verbs + points instead of `BezPath` — declined.** `BezPath` is the
  public geometry type (`pub use pdfrum::BezPath`, `PathBuilder`, `PathObject`,
  `ClipEntry`), and all three rasterizer backends consume kurbo (`vello_cpu`
  takes `kurbo::BezPath` directly; `tiny-skia` builds its own path from it).
  A private path type would be converted back at every backend boundary.
  Flattening (`kurbo::bezpath::flatten`, 6.3% on `vector_paths_1751`) is
  compute, not layout. Nothing in the profile is path-memory-bound.

## 2. What the profile found instead — the M18 items

The proposal's instinct was right about the build being the cold-render cost;
the layout it blamed was wrong. Two concrete traffic problems account for
most of both documents' build time, and both are DOD in the sense that
matters — memory touched per unit of work — without moving a public type.

### 2.1 `Face::name_index` builds the name map once

`crates/pdfrum-font/src/glyphs/face.rs:379-395`: for every name looked up,
walk `0..num_glyphs` calling `post.glyph_name(gid)`, itself O(gid) for a
`post` 2.0 table. A simple font with `/Differences` looks up up to 256 names,
so a 3 000-glyph face costs on the order of 10⁹ byte reads. Fix: one pass
over `post.glyph_names()` (read-fonts' iterator, which carries checkpoints
for exactly this) into a `HashMap<Box<str>, u16>` (or a sorted `Vec`) held
lazily on the `Face`; the CFF charset branch (`cff_name_index`) gets the
same treatment. Expected: `vector_en_tem` cold −50% or better (58% of `Ir`),
and every text document with a named-encoding TrueType font moves with it.
Correctness pin: the map answers what the scan answered — the **first** gid
with that name — for every face in the corpus (a test that builds both and
compares, kept under `cfg(test)`).

### 2.2 A zero-copy content lexer

`crates/pdfrum-page/src/tokenize.rs`: `next_word` returns `(Vec<u8>, bool)`
by `to_vec()`; the lexer holds `last_word: Vec<u8>`; numbers go through
`std::str::from_utf8` and `str::parse::<f32>`. The lexer becomes borrowing
(`&'a [u8]` slices of the decoded content, which `content_segments` already
concatenates into one buffer), strings and names that need unescaping keep
one reusable scratch buffer, and numbers parse from bytes. **The number
parse must be bit-identical**: `word_to_number`'s digit rules are oracle
behaviour (`docs/status/pdfrum-page.md` §"word_to_number"), so the new path
keeps `parse_real`'s semantics and is pinned by a test that runs old and new
over every number token in every corpus content stream. Expected on
`vector_paths_1751`: build −30% to −40% (lexer 46.8% of `Ir`, allocation
~12% self), with the `build/*` bench group as the ratchet.

### 2.3 `Object::Stream(Box<Stream>)`

§1.3 A. Small, mechanical (`Object::Stream(s)` patterns become
`Object::Stream(s)` with `*s` where the value is moved out; constructors box),
landed with `api-snapshot update` and an RSS row.

### 2.4 The clip-stack count

§1.3 B. A measurement with a threshold; a one-line change if it trips.

## 3. Rules the pass runs under

- **No new dependency** (DEPS.md "Performance ring": tuned no-dep first, and
  none of the four items needs one).
- **Conformance byte-identical** after every commit (1757 rows; the board
  recipe in `docs/status/queue.md`).
- **Every claim carries an `Ir` before/after and a ratchet run**, interleaved,
  from one target dir per sha (`docs/status/M13-perf-baseline.md` §18 method).
  Wall-clock on this machine is quoted with its load.
- **Public types move only where §2.3 says**, and `api-snapshot update` is
  a deliberate commit of its own.
- **Style**: no `unsafe`, no sentinels, no lifetimes in public types
  (the borrowing lexer is `pub(crate)`).

## 4. Exit

- `vector_en_tem` cold: build `Ir` ≤ 50% of today's; `render-cold-*` entries
  for every text-class document not slower.
- `vector_paths_1751`: `build/` entry −25% or better; `render-warm-*` unmoved.
- `Object` 32 bytes; RSS on the ten largest corpus files recorded before and
  after; `open/*` inside its band.
- Clip-stack count table for the corpus in the status doc, with the
  threshold decision.
- `docs/status/M18.md` records each item's numbers, and this document's
  §1.3 verdicts stand or are corrected in place.
