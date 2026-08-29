# pdfrum-bench

What pdfrum costs, and the machinery that keeps it from getting worse. Nothing
here is published (`publish = false`) and no library crate depends on it.

`docs/status/M12.md` carries the numbers and the reasoning behind every
threshold in this directory; this file is the operating manual.

## Layout

Since M12 the suite is **split per crate**: each library crate owns the
benchmark for its own code, and this directory holds the parts that are about
the corpus as a whole. `cargo bench -p pdfrum-render` is a command with a
meaning; before the split, measuring the rasterizer meant building and running
everything.

| Path | What it is |
|---|---|
| `corpus/` | The 44 measurement documents, six classes. `corpus/PROVENANCE.md` says where each came from and why it is in the set. |
| `corpus-list/` | The `pdfrum-corpus` crate: the one list of documents and classes. A **leaf** — it depends on nothing, which is what lets five library crates share it without depending on the facade that depends on them. |
| `fixtures/` | The seven M8 documents, kept so `docs/status/M8.md`'s tables stay reproducible. Not the M12 measurement set. |
| `src/bin/ratchet.rs` | Compares a run against `baseline.json`; fails on a regression outside the noise band. |
| `src/bin/profile.rs` | One operation in a loop with nothing else in the process, for `scripts/profile.sh`. |
| `src/bin/scaling.rs` | One rayon thread count per process, for `scripts/bench-scaling.sh`. |
| `baseline.json` | The committed medians, and the per-group noise bands they are judged with. |

The eleven criterion groups and who owns them:

| Crate | Bench file | Groups |
|---|---|---|
| `pdfrum-parser` | `benches/open.rs` | `open` |
| `pdfrum-page` | `benches/build.rs` | `build` |
| `pdfrum-render` | `benches/render.rs` | `render-cold-{exact,tinyskia,vello}`, `render-warm-{exact,tinyskia,vello}` |
| `pdfrum-text` | `benches/text.rs` | `text` |
| `pdfrum-edit` | `benches/save.rs` | `save` |

## Cold and warm are two different questions

The render groups come in pairs and the distinction is the whole of M12's
final correction. **`render-cold`** builds a fresh `RenderSession` inside the
timed closure: first-render latency, what a caller who opens a document, draws
one page and exits waits for. **`render-warm`** holds one session across
iterations: steady-state, what a viewer scrolling or a server rendering the
same page twice pays from the second render on.

The oracle's column is a **warm** number — `pdfium_test --render-repeats`
renders in one process with `CPDF_PageImageCache` on by default — so the M12
exit target is judged on `render-warm`. `render-cold` is tracked with its own
ratchet entries and no oracle target. `docs/status/M12.md` §1.8 has the
argument; the short version is that comparing our cold path against their warm
one was measuring our worst case against their best and calling the quotient a
speed ratio.

## Running it

```sh
# The dev loop: 18 documents, criterion's floor, ~3 minutes. Not a ratchet run.
scripts/bench-quick.sh
scripts/bench-quick.sh render-warm      # or one group

# The full suite. ~1 hour: 44 documents x 11 groups.
cargo bench --workspace

# One crate's groups.
cargo bench -p pdfrum-render

# Did anything get slower?
cargo run --release -p pdfrum-bench --bin ratchet -- check

# Record the improvements (regressions still fail and write nothing).
cargo run --release -p pdfrum-bench --bin ratchet -- update

# Where does one document's time go?
scripts/profile.sh render benches/corpus/text_foxittext.pdf 50 exact

# How does it scale across cores?
scripts/bench-scaling.sh 1 2 4 8 16 32

# What does the oracle take on the same files?
scripts/bench-oracle.sh
```

A filtered run is fine for iterating —
`cargo bench -p pdfrum-render -- text_foxittext` — and the ratchet will tell
you which benchmarks it therefore did **not** check rather than reporting a
clean sweep over a subset. One caveat the ratchet cannot warn about: a
single-crate run leaves the *other* crates' results on disk from whenever they
last ran, and `check` will compare against those as if they were fresh. Run
`cargo bench --workspace` before a check that decides anything.

## Three things worth knowing before trusting a number

**The machine matters more than the change.** Run-to-run spread on an unchanged
binary reaches 5% on the heavier documents. The ratchet's per-group bands (in
`baseline.json`) are set from measurement, and a result inside the band is *no
result*. Close the browser before taking a number you intend to commit.

**`perf` may not be available, and the harness will say so.** It needs
`kernel.perf_event_paranoid <= 1`; `scripts/profile.sh` checks, prints the
`sysctl` that fixes it, and otherwise falls back to exact instrumentation at the
engine/rasterizer seam. That fallback is not a sampler and does not pretend to
be one — it cannot name a symbol, only a phase.

**A perf change in a parity engine has to prove itself twice.** Faster is not
enough: `conformance run --check-regressions` must show the same scoreboard, and
for anything touching the compositor or the blitter, expect to write the
exhaustive equivalence test as well. `crates/pdfrum-render/src/blend.rs`'s
`the_opaque_normal_fast_path_is_exhaustively_identical` is the pattern.

## Adding a document to the corpus

1. Copy it in unmodified, named `<class>_<subject>.pdf`.
2. Add a row to `corpus/PROVENANCE.md` — source path, licence, why it is here.
3. Add it to `CORPUS` in `src/corpus.rs` with its class and page count.
4. `cargo bench` and `ratchet update`: a document with no committed number is
   reported as *new*, not as a failure.

Choose it on **measured cost and dominant operator**, not on file size. The M12
scan of all 1375 oracle files found a 25 MB document that renders in 90 ms and a
1 KB one that takes 1.2 seconds; size predicts almost nothing here.
