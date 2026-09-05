# Cold start, measured — what is inside the benchmark's cold number

Measured 2026-09-06 on `frieren` (32 CPUs, **not** idle — load average 14–21
throughout), so every absolute millisecond below is a ratio's denominator, not
a quotable figure. The comparisons are all back-to-back A/B on the same box
within minutes of each other.

The question this answers: `docs/benchmarks/README.md` run 3 records pdfrum's
cold render median on the 44-file corpus at 41.90 ms against a 10.55 ms warm
median — a 4x cold/warm ratio where mupdf, pdfium-render and hayro sit at
1.5–3.6x. Yet `docs/design/mupdf-comparison.md` §0 measured a single cold
render within 15% of mupdf per page in callgrind `Ir`. Both are true, and this
document says why.

## 0. What the cold number actually contains

`benches/compare/src/model.rs` `Ctx::measure` times the closure it is handed,
once cold and then up to three times warm. For `Op::Render`,
`benches/compare/src/engines/pdfrum.rs` does the open, `page(0)`, the backend
and the `RenderSession` **outside** that closure:

```rust
let doc = open(path, ctx)?;
let page = doc.page(0)?;
let backend = VelloCpuBackend::new();
let options = RenderOptions::scaled(ctx.scale());
let mut session = session(&doc, ctx);
let (times_ms, pixmap) =
    ctx.measure(|| Ok(page.render_on(&backend, &options, &mut session)?))?;
```

So the cold number is **only the first `render_on`**. Parsing, session
construction and process start are not in it. The cold/warm gap is therefore
entirely *lazy work that the first render triggers and the later ones reuse* —
and, decisively, the harness spawns a fresh child process per file, so
"once per process" is once per measured number.

The harness's cold definition is **not wrong** and was not changed. The
README's "Time, memory, isolation" section describes it accurately.

## 1. The cause, with numbers: the font-directory scan

Every benchmark `run` invocation passes `--font-dir <checkout>/third_party/
test_fonts` (`benches/compare/src/oracle.rs` `determinism_args`), so the
hermetic scan — not the host scan — is the one on the benchmark's path.

That scan is triggered lazily by the first font substitution, which happens
*inside* the first `render_on`. `fontdb` enumerates the directory and, because
its own memory map does not outlive its loader, hands back `Source::File(path)`
with no bytes; `SystemFontDb::scan` then reads each face whole so `describe`
can read its `name` and `OS/2` tables.

**The oracle's `test_fonts` is 33.8 MB across 31 files**, and 26 MB of that is
two faces the ladder rarely picks (`NotoSansCJKjp-Regular.otf` 16.4 MB,
`NotoColorEmoji.ttf` 9.4 MB). Both appear in the trace as single whole-file
`read` calls.

Syscall time for one cold render of `vector_en_tem`, `strace -f -c`, same
binary, same file, back to back:

| | syscalls | syscall time |
|---|---|---|
| with `--font-dir` | 586 | **20.5 ms** |
| without | 144 | **2.9 ms** |

A 17.6 ms delta, of which 84% is 89 `read` calls averaging 143 us. This is
**kernel time on a warm page cache**, which is exactly why a per-page
instruction profile cannot see it: in `callgrind --inclusive=yes` over the same
cold render, the whole substitution path is under 1% of `Ir`, while
`Page::render_on` is 216 M `Ir` (44%).

### On the whole corpus

Counted with strace over all 44 files: **22 of the 44 pay exactly one
font-directory scan** inside their timed cold render. None pays more than one.

Cold and warm medians over all 44 files, `--warm 3`, one binary, the two runs
minutes apart (load 19.4 → 14.4):

| | cold median | warm median | ratio |
|---|---|---|---|
| with `--font-dir` (what run 3 measures) | **45.42 ms** | 12.55 ms | **3.62x** |
| without `--font-dir` | **33.21 ms** | 14.13 ms | **2.35x** |

The scan is worth **~12 ms of the corpus cold median** and moves the cold/warm
ratio from 2.35x — squarely inside the peers' 1.5–3.6x band — to 3.62x. It is
the single largest identified cold-only cost.

## 2. Like for like: hayro does not scan

`strace -f -e trace=openat` over hayro's cold render of the same file: **zero
font files opened**. hayro bundles its fonts and never consults the system or
any directory. Its cold/warm ratio on `vector_en_tem` was 8.08 / 4.10 = 2.0x
against our 31.2 / 6.75 = 4.6x on the same box in the same minute.

So the comparison in run 3 is not measuring the same work on both sides: our
number includes a hermetic-font-set scan that exists to match the oracle's
substitution, and hayro's does not.

## 3. What was fixed, and what was not

**Fixed** (`crates/pdfrum-font/src/subst/mod.rs`, commit "a --font-dir scan is
cached per process"): the host scan was already memoized in a `OnceLock`, but a
`--font-dir` scan was rebuilt per call, so a document substituting two fonts
scanned twice. A `ScanKey` now names the directory list and one process-wide map
answers both. Honestly reported: **this does not move the 44-file corpus
number**, because no corpus file substitutes twice. It bounds a cost that was
previously proportional to a document's substituted-font count.

**Not fixed, and why.** The obvious remaining fix is to stop reading whole font
files during the scan: `describe` wants two small tables and the code faults in
33 MB to find them. The natural implementation is to memory-map each file
instead. That was written and then **backed out**: `crates/pdfrum-font/src/lib.rs`
carries `#![forbid(unsafe_code)]` (and the workspace sets
`unsafe_code = "forbid"`), and `memmap2::Mmap::map` is unsafe. Weakening a
stated crate-wide safety invariant to improve a benchmark is the wrong trade,
and it is not this agent's call to make.

The safe alternative — parse the sfnt table directory by hand, read only the
`name` and `OS/2` tables by offset, and hand skrifa a reconstructed minimal
sfnt — is real work with a real risk: any slip changes which face the ladder
picks, across 1759 board files. It is left as a queued item with the numbers
above attached, so whoever takes it knows it is worth ~12 ms of the corpus cold
median and nothing else in the engine.

## 4. What is not the cause

- **Not the standard-14 faces.** They are `include_bytes!` constants parsed
  behind per-variant `OnceLock`s; they do not appear meaningfully in the `Ir`
  profile.
- **Not process start or parsing.** Both are outside the timed closure.
- **Not the engine's instruction count.** `mupdf-comparison.md` §0 stands: per
  page, in `Ir`, we are where it said we were. The cold gap is kernel time, and
  an `Ir` profile is blind to it by construction. That is the correction this
  document makes to the earlier reading.

One more measurement worth recording, though it is the harness's and not the
engine's: in the same callgrind run, `pixels::encode_png` is **207 M `Ir`, 42%
of the whole child process** — the harness writing its output PNG, outside the
timed region. It costs nothing in the reported numbers but dominates the
profile, and anyone profiling a child process should expect to see it.

## Board

1759 files, 1547 pass, 212 fail — **0 changed rows** against
`board-pathclose-main.json`. The substitution ladder's tests in `pdfrum-font`
pass unchanged (1436 tests, 1436 passed).
