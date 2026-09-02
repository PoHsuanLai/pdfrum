#!/usr/bin/env nu
# Profile one pdfrum operation on one file, and say where the time goes.
#
# Usage:
#   scripts/profile.nu <op> <file.pdf> [iterations] [backend] [--walk]
#   scripts/profile.nu render benches/fixtures/foxittext.pdf 50 exact
#   scripts/profile.nu render benches/corpus/text_foxittext.pdf 30 exact --walk
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

def main [
    op: string = render
    file: path = benches/fixtures/foxittext.pdf
    iterations: int = 50
    backend: string = exact
    --walk
] {
    cd ($env.FILE_PWD | path dirname)

    if not ($file | path exists) {
        print --stderr $"error: no such file: ($file)"
        exit 1
    }

    let stem = ($file | path basename | str replace --regex '\.pdf$' '')
    let out = 'target/profile'
    mkdir $out

    # Debug symbols in the release profile, so a profile names functions rather
    # than addresses. `[profile.release] debug = 1` in the root Cargo.toml keeps
    # this free of a separate build; if it is ever removed, this is where the
    # profile goes blind.
    print "==> building pdfrum-bench --release"
    let features = if $walk { [--features walk-profile] } else { [] }
    ^cargo build --release -p pdfrum-bench --bin profile ...$features | ignore

    let target = (do --ignore-errors {
        ^cargo metadata --format-version 1 --no-deps | from json | get target_directory
    })
    let bin = ([($target | default 'target') release profile] | path join)
    if not ($bin | path exists) {
        print --stderr $"error: built binary not found at ($bin)"
        exit 1
    }

    let args = [--op $op --file $file --iterations ($iterations | into string) --backend $backend]

    # `--walk` is about the engine half's internal split, which `perf` does not
    # answer any better than the timers do — and a `perf record` over an already
    # instrumented binary reports the instrument. So it takes the in-process
    # path unconditionally.
    let have_perf = if $walk {
        print --stderr "note: --walk uses the in-process split; skipping perf."
        false
    } else if not (which perf | is-empty) {
        let paranoid = (do --ignore-errors {
            open /proc/sys/kernel/perf_event_paranoid | str trim | into int
        } | default 4)
        if $paranoid <= 1 {
            true
        } else {
            print --stderr $"note: perf is installed but kernel.perf_event_paranoid=($paranoid)"
            print --stderr "      hardware sampling needs <= 1. To enable for this boot:"
            print --stderr "        sudo sysctl kernel.perf_event_paranoid=1"
            print --stderr "      falling back to the in-process sampler."
            false
        }
    } else {
        print --stderr "note: 'perf' is not installed; falling back to the in-process sampler."
        print --stderr "      For the full profile install it (Debian/Ubuntu:"
        print --stderr '        sudo apt-get install linux-tools-common linux-tools-$(uname -r))'
        false
    }

    if not $have_perf {
        print $"==> in-process sampler \(($op), ($stem), ($iterations)x, backend=($backend)\)"
        ^$bin ...$args --sample
        return
    }

    let data = ([$out $"($op)-($stem).data"] | path join)
    print $"==> perf record \(($op), ($stem), ($iterations)x, backend=($backend)\)"
    ^perf record -F 999 --call-graph dwarf,16384 -g -o $data -- $bin ...$args

    print ""
    print "==> flat profile (self time, top 40)"
    do --ignore-errors {
        ^perf report -i $data --no-children --percent-limit 0.2 --stdio
    } | lines | where {|l| $l =~ '^ +[0-9]' } | first 40 | each {|l| print $l } | ignore

    let svg = ([$out $"($op)-($stem).svg"] | path join)
    if not (which inferno-flamegraph | is-empty) {
        do --ignore-errors {
            ^perf script -i $data | ^inferno-collapse-perf | ^inferno-flamegraph | save --force $svg
            print $"flamegraph: ($svg)"
        }
    } else if not (which flamegraph.pl | is-empty) {
        do --ignore-errors {
            ^perf script -i $data | ^stackcollapse-perf.pl | ^flamegraph.pl | save --force $svg
            print $"flamegraph: ($svg)"
        }
    } else {
        print --stderr "note: no inferno-flamegraph / flamegraph.pl; skipping the SVG."
        print --stderr "      cargo install inferno   # pure Rust, tool ring only"
    }
}
