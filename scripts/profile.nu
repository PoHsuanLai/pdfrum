#!/usr/bin/env nu
# Profile one pdfrum operation on one file.
#
# scripts/profile.nu <op> <file.pdf> [iterations] [backend] [--walk] [--warm]
#
# op render | text | open | save | forms
# backend agg | tiny-skia | vello-cpu (render only; default agg)
# --warm hold one RenderSession across iterations (matches
# pdfium_test --render-repeats). Use this for oracle-relative
# figures.
# --walk in-process stage split. Read shares from --walk; absolute
# milliseconds from a plain run. The timers cost time.
#
# `--sample` (the profile binary) builds the page graph once and times the
# engine; the plain loop rebuilds it every iteration. A/B engine work on
# --sample.
#
# `--op forms` is annotations on vs off over forms_*.pdf — the appearance
# overlay, not the interaction path. Ignores `--warm`.
#
# Falls back from `perf record` to the in-process sampler when perf is
# unavailable.

def main [
    op: string = render
    file: path = benches/fixtures/foxittext.pdf
    iterations: int = 50
    backend: string = agg
    --walk
    --warm
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
    let features = if $walk { [--features profiling] } else { [] }
    ^cargo build --release -p pdfrum-bench --bin profile ...$features | ignore

    let target = (do --ignore-errors {
        ^cargo metadata --format-version 1 --no-deps | from json | get target_directory
    })
    let bin = ([($target | default 'target') release profile] | path join)
    if not ($bin | path exists) {
        print --stderr $"error: built binary not found at ($bin)"
        exit 1
    }

    let args = (
        [--op $op --file $file --iterations ($iterations | into string) --backend $backend]
        | append (if $warm { [--warm] } else { [] })
    )

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
        # Two ops decline `--sample` here rather than being handed it and having
        # the result explained away.
        #
        # `--op forms` prints its own two-arm split and has no engine/raster seam
        # to add one to. And `--warm` on a render is asking for the *whole-document
        # figure the oracle is comparable with*, which is what the plain loop
        # prints; `--sample` would instead take the `timed_render` path, whose
        # loop hoists the page graph and so answers the other question entirely
        # and it would do so silently, which is exactly the confusion the
        # "WARM, COLD" section above exists to prevent. Asking for both gets the
        # warm total.
        let declines_sample = ($op == forms) or ($op == render and $warm)
        if $declines_sample {
            ^$bin ...$args
        } else {
            ^$bin ...$args --sample
        }
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
