# pdfrum-bench

What pdfrum costs, and the machinery that keeps it from getting worse. Nothing
here is published (`publish = false`) and no library crate depends on it.

`docs/status/M12.md` carries the numbers and the reasoning behind every
threshold in this directory; this file is the operating manual.

## Layout

| Path | What it is |
|---|---|
| `corpus/` | The 44 measurement documents, six classes. `corpus/PROVENANCE.md` says where each came from and why it is in the set. |
| `fixtures/` | The seven M8 documents, kept so `docs/status/M8.md`'s tables stay reproducible. Not the M12 measurement set. |
| `src/corpus.rs` | The one list of documents and classes, shared by everything below, so a table row means the same thing in every column. |
| `benches/engine.rs` | The criterion suite: `open`, `render-{exact,tinyskia,vello}`, `text`, `save`. |
| `src/bin/ratchet.rs` | Compares a run against `baseline.json`; fails on a regression outside the noise band. |
| `src/bin/profile.rs` | One operation in a loop with nothing else in the process, for `scripts/profile.sh`. |
| `src/bin/scaling.rs` | One rayon thread count per process, for `scripts/bench-scaling.sh`. |
| `baseline.json` | The committed medians, and the per-group noise bands they are judged with. |

## Running it

```sh
# The full suite. ~40 minutes: 44 documents x 6 groups.
cargo bench -p pdfrum-bench

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
`cargo bench -p pdfrum-bench -- text_foxittext` — and the ratchet will tell you
which benchmarks it therefore did **not** check rather than reporting a clean
sweep over a subset.

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
