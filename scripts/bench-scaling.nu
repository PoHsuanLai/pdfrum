#!/usr/bin/env nu
# The rayon multi-page scaling curve: how much faster does a document render
# when its pages go to N workers instead of one?
#
# Usage: scripts/bench-scaling.nu [threads...]
#        scripts/bench-scaling.nu 1 2 4 8 16 32
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
# same argument `scripts/bench-oracle.nu` makes at more length.

def main [...threads: int] {
    cd ($env.FILE_PWD | path dirname)

    let threads = if ($threads | is-empty) { [1 2 4 8 16] } else { $threads }

    print "==> building"
    ^cargo build --release -p pdfrum-bench --bin scaling | ignore

    let target = (do --ignore-errors {
        ^cargo metadata --format-version 1 --no-deps | from json | get target_directory
    })
    let bin = ([($target | default 'target') release scaling] | path join)

    if not ($bin | path exists) {
        print --stderr $"error: no scaling binary at ($bin)"
        exit 1
    }

    print $"machine: (sys cpu | length) logical CPUs"
    print ""

    cd benches
    for n in $threads {
        ^$bin --threads $n --rounds 3
    }
}
