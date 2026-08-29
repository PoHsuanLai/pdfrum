#!/usr/bin/env bash
# The rayon multi-page scaling curve: how much faster does a document render
# when its pages go to N workers instead of one?
#
# Usage: scripts/bench-scaling.sh [threads...]
#        scripts/bench-scaling.sh 1 2 4 8 16 32
#
# Runs `benches`' `scaling` binary over every corpus document with eight pages
# or more (`pdfrum_bench::corpus::multipage`), once per thread count, and prints
# a speed-up curve per document and a geometric mean across them.
#
# WHY A SCRIPT AND NOT A CRITERION GROUP
#
# rayon's pool size is process-global and fixed at first use: `ThreadPoolBuilder`
# can only be built once per process, so a criterion group cannot sweep it — the
# second configuration would silently reuse the first's pool and report a flat
# curve that looks like a result. One process per thread count is the only
# honest way to measure this, and that is a script's shape rather than a
# benchmark harness's.
#
# WHAT THE NUMBER MEANS
#
# The serial baseline is the *same binary* with `--threads 1`, not a separate
# single-threaded build, so the comparison isolates parallelism from every other
# difference. Each measurement is the best of three runs: a wall-clock sample is
# bounded below by the real cost and unbounded above by whatever else the
# machine was doing, so the minimum is the least contaminated estimate — the
# same argument `scripts/bench-oracle.sh` makes at more length.
set -euo pipefail
cd "$(dirname "$0")/.."

THREADS=("$@")
if [ ${#THREADS[@]} -eq 0 ]; then
    THREADS=(1 2 4 8 16)
fi

echo "==> building"
cargo build --release -p pdfrum-bench --bin scaling >/dev/null

BIN=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
    | tr ',' '\n' | grep -o '"target_directory":"[^"]*"' | head -1 \
    | sed 's/.*:"//;s/"$//')
BIN=${BIN:-target}/release/scaling

if [ ! -x "$BIN" ]; then
    echo "error: no scaling binary at $BIN" >&2
    exit 1
fi

echo "machine: $(nproc) logical CPUs"
echo

cd benches
for n in "${THREADS[@]}"; do
    "$BIN" --threads "$n" --rounds 3
done
