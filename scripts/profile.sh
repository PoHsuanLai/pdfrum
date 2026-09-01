#!/usr/bin/env bash
# Profile one pdfrum operation on one file, and say where the time goes.
#
# Usage:
#   scripts/profile.sh <op> <file.pdf> [iterations] [backend] [--walk]
#   scripts/profile.sh render benches/fixtures/foxittext.pdf 50 exact
#   scripts/profile.sh render benches/corpus/text_foxittext.pdf 30 exact --walk
#
#   op        render | text | open | save   (see `pdfrum-bench --help`)
#   file      a PDF; relative paths resolve from the workspace root
#   iterations how many times to repeat the op inside one process (default 50)
#   backend   exact | tinyskia | vello   (render only; default exact)
#   --walk    also split the ENGINE half into the walk's own phases, and count
#             the walk's per-object allocations by site. Builds with
#             `pdfrum-render/walk-profile` and forces the in-process
#             instrumentation path even where `perf` is available, because the
#             two answer different questions. **The timers cost real time** —
#             about a third of a path-heavy render, all of it `Instant::now()`
#             pairs — so read shares from a `--walk` run and absolute
#             milliseconds from a plain one. See docs/status/M12b-P2.md §3.
#
# Output: a flat symbol profile on stdout, and — when the tooling is present —
# `target/profile/<op>-<stem>.svg`, a flamegraph.
#
# THE TWO RENDER LOOPS, AND WHICH ONE A/Bs THE ENGINE
#
# `profile --op render` has two loops and they measure different things. The
# plain one builds a fresh `RenderSession` per iteration and calls
# `page.render_session()`, which **re-derives the page graph every iteration**;
# on a content-heavy file that is mostly `pdfrum-page`. `--sample` builds the
# graphs once outside the loop, reports the build separately, and reuses one
# `RenderCaches` — its TOTAL is the render.
#
# The gap is not small. `vector_en_tem` reports 34 ms plain and 6.2 ms under
# `--sample`; 82% of the plain figure is the rebuild. An A/B of an engine change
# run on the plain loop is therefore mostly an A/B of the parser, and
# docs/status/M12b-P3.md §4 records one that produced a reproducible +5%
# "regression" which survived a bisection and did not exist.
#
# **A/B engine work on `--sample`.** Use the plain loop when the page build is
# part of what you mean to measure.
#
# WHY A DEDICATED BINARY AND NOT `cargo bench`
#
# criterion's harness is itself several percent of a profile and, worse, it
# interleaves its own statistics between iterations, so the samples that land
# in the engine are diluted by samples that land in criterion's bookkeeping.
# `pdfrum-bench`'s `profile` binary runs the same code path the corresponding
# criterion group runs, in a loop, with nothing else in the process — so a
# symbol's share of the profile is its share of the operation.
#
# PERF AVAILABILITY
#
# `perf record` is the default and needs two things: the binary on PATH, and
# `kernel.perf_event_paranoid <= 1` (or CAP_PERFMON). This script checks both
# and, when either is missing, says exactly which and falls back rather than
# failing: with `perf` absent it reports the requirement and runs the operation
# under the in-process sampler instead (`--sample`, a SIGPROF-driven backtrace
# sampler built into the profile binary, no dependency), which gives a coarser
# but real answer. The flamegraph step is skipped silently when
# `inferno-flamegraph` or `flamegraph.pl` is not installed — the flat profile
# is the part that matters and the SVG is a convenience.
set -euo pipefail
cd "$(dirname "$0")/.."

WALK=0
ARGV=()
for arg in "$@"; do
    if [ "$arg" = "--walk" ]; then WALK=1; else ARGV+=("$arg"); fi
done
set -- "${ARGV[@]+"${ARGV[@]}"}"

OP=${1:-render}
FILE=${2:-benches/fixtures/foxittext.pdf}
ITERS=${3:-50}
BACKEND=${4:-exact}

if [ ! -f "$FILE" ]; then
    echo "error: no such file: $FILE" >&2
    exit 1
fi

STEM=$(basename "$FILE" .pdf)
OUT=target/profile
mkdir -p "$OUT"

# Debug symbols in the release profile, so a profile names functions rather
# than addresses. `[profile.release] debug = 1` in the root Cargo.toml keeps
# this free of a separate build; if it is ever removed, this is where the
# profile goes blind.
echo "==> building pdfrum-bench --release"
FEATURES=()
if [ "$WALK" = 1 ]; then
    FEATURES=(--features walk-profile)
fi
cargo build --release -p pdfrum-bench --bin profile "${FEATURES[@]+"${FEATURES[@]}"}" >/dev/null

BIN=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
    | tr ',' '\n' | grep -o '"target_directory":"[^"]*"' | head -1 \
    | sed 's/.*:"//;s/"$//')
BIN=${BIN:-target}/release/profile
if [ ! -x "$BIN" ]; then
    echo "error: built binary not found at $BIN" >&2
    exit 1
fi

ARGS=(--op "$OP" --file "$FILE" --iterations "$ITERS" --backend "$BACKEND")

# `--walk` is about the engine half's internal split, which `perf` does not
# answer any better than the timers do — and a `perf record` over an already
# instrumented binary reports the instrument. So it takes the in-process path
# unconditionally.
have_perf=0
if [ "$WALK" = 1 ]; then
    echo "note: --walk uses the in-process split; skipping perf." >&2
elif command -v perf >/dev/null 2>&1; then
    paranoid=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 4)
    if [ "$paranoid" -le 1 ]; then
        have_perf=1
    else
        echo "note: perf is installed but kernel.perf_event_paranoid=$paranoid" >&2
        echo "      hardware sampling needs <= 1. To enable for this boot:" >&2
        echo "        sudo sysctl kernel.perf_event_paranoid=1" >&2
        echo "      falling back to the in-process sampler." >&2
    fi
else
    echo "note: 'perf' is not installed; falling back to the in-process sampler." >&2
    echo "      For the full profile install it (Debian/Ubuntu:" >&2
    echo "        sudo apt-get install linux-tools-common linux-tools-\$(uname -r))" >&2
fi

if [ "$have_perf" = 1 ]; then
    DATA=$OUT/$OP-$STEM.data
    echo "==> perf record ($OP, $STEM, ${ITERS}x, backend=$BACKEND)"
    perf record -F 999 --call-graph dwarf,16384 -g -o "$DATA" -- "$BIN" "${ARGS[@]}"

    echo
    echo "==> flat profile (self time, top 40)"
    perf report -i "$DATA" --no-children --percent-limit 0.2 --stdio 2>/dev/null \
        | grep -E '^ +[0-9]' | head -40 || true

    if command -v inferno-flamegraph >/dev/null 2>&1; then
        SVG=$OUT/$OP-$STEM.svg
        perf script -i "$DATA" 2>/dev/null \
            | inferno-collapse-perf 2>/dev/null \
            | inferno-flamegraph > "$SVG" 2>/dev/null && echo "flamegraph: $SVG"
    elif command -v flamegraph.pl >/dev/null 2>&1; then
        SVG=$OUT/$OP-$STEM.svg
        perf script -i "$DATA" 2>/dev/null \
            | stackcollapse-perf.pl 2>/dev/null \
            | flamegraph.pl > "$SVG" 2>/dev/null && echo "flamegraph: $SVG"
    else
        echo "note: no inferno-flamegraph / flamegraph.pl; skipping the SVG." >&2
        echo "      cargo install inferno   # pure Rust, tool ring only" >&2
    fi
else
    echo "==> in-process sampler ($OP, $STEM, ${ITERS}x, backend=$BACKEND)"
    "$BIN" "${ARGS[@]}" --sample
fi
