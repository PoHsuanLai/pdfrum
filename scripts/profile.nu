#!/usr/bin/env nu
# Profile one pdfrum operation on one file, and say where the time goes.
#
# Usage:
#   scripts/profile.nu <op> <file.pdf> [iterations] [backend] [--walk] [--warm]
#   scripts/profile.nu render benches/fixtures/foxittext.pdf 50 agg
#   scripts/profile.nu render benches/corpus/text_foxittext.pdf 30 agg --walk
#   scripts/profile.nu forms benches/corpus/forms_push_button.pdf 20
#
#   op        render | text | open | save | forms  (see `pdfrum-bench --help`)
#   file      a PDF; relative paths resolve from the workspace root
#   iterations how many times to repeat the op inside one process (default 50)
#   backend   agg | tinyskia | vello-cpu   (render only; default agg. The old
#             spellings `exact` and `vello` are still accepted by the binary:
#             the crates were renamed `pdfrum-raster-exact` -> `-agg` and
#             `-vello` -> `-vello-cpu`, and the aliases keep the commands in
#             docs/status/M12*.md runnable.)
#   --warm    hold ONE `RenderSession` across every iteration instead of
#             building a fresh one per iteration. This is the difference
#             between the `render-cold-*` and `render-warm-*` criterion groups,
#             and it is the flag that makes a `--op render` figure comparable
#             with `pdfium_test --render-repeats`. See "WARM, COLD" below.
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
# WARM, COLD, AND WHICH ONE COMPARES WITH THE ORACLE
#
# The section above is about the *page graph*, which is a question inside our
# own engine. This one is about the *caches*, which is the question that decides
# whether a number may be divided by the oracle's.
#
# Without `--warm`, `--op render` builds a fresh `RenderSession` inside the
# loop, so every iteration pays a cold `BuildContext` (fonts, colour spaces,
# decoded images) and a cold `RenderCaches` (flattened glyph outlines). That is
# `render-cold-*`. With `--warm`, one session is built and primed with an
# untimed render before the loop and held across every iteration. That is
# `render-warm-*`.
#
# **The oracle is warm.** `pdfium_test --render-repeats=n` loops
# `ProcessPage(i)` n times inside ONE process (`pdfium_test.cc:1804`), so each
# repetition re-loads and re-renders the page while `CPDF_PageImageCache` and
# the font database stay warm across all n. Dividing a cold pdfrum figure by
# that column measures our worst case against their best one and calls the
# quotient a speed ratio; docs/status/M12.md §1.8 is the correction, and it is
# what moved the published geomean from 3.58x to 0.97x with no code change.
#
# So: **every oracle-relative figure is taken with `--warm`.** Note what warm
# does NOT mean — the page graph is still rebuilt on every iteration, the
# content stream re-walked, every path re-flattened, on both sides. `--warm`
# and `--sample` are therefore not the same hoist and are not interchangeable:
# `--sample` hoists the *graph* and answers "which half of our engine", `--warm`
# hoists the *caches* and answers "how fast are we against them". Use `--sample`
# for an A/B of an engine change; use `--warm` for a ratio.
#
# ONE TRAP THIS SCRIPT USED TO FALL INTO, FIXED 2026-09-02
#
# `--op render`'s clock used to start before the document was parsed, which was
# harmless while every op re-parsed, and stopped being harmless the moment
# `--warm` added a priming render in front of the loop. The priming pass is a
# *cold* render by construction, and on `image_bug_718762` a cold render is
# ~1.2 s against a warm one of well under a millisecond — so at eight
# iterations the reported per-iteration figure was about 99% priming, and the
# giveaway was that it fell with the iteration count instead of converging:
# 146 ms at n=8, 18 ms at n=100, 11.6 ms at n=400, on one document, one binary,
# one machine. The clock now starts after setup for every op but `open`, whose
# operation *is* the parse. If a `--warm` figure ever scales with `--iterations`
# again, this is the first thing to check.
#
# THE `forms` OP: WHAT IT MEASURES AND WHY IT IS THIS AND NOT `pdfrum-form`
#
# docs/status/M12.md §11 names `forms` at 3.35x warm as the milestone's largest
# residue, and docs/status/M12b.md §9 records that no item since has re-measured
# it. Four ops existed and none of them could see inside that class: `render`
# renders a forms document exactly as it renders any other, so it reproduces the
# number without decomposing it.
#
# The obvious guess for a "forms operation" is the interaction path — field
# construction, the event cascade, `pdfrum_form::apply` — because that is what
# the conformance `#form-events` rows drive. **That guess is wrong for this
# number**, and the reason is worth stating so nobody re-makes it: the 3.35x is
# a *render* ratio taken over `benches/corpus/forms_*.pdf` against
# `pdfium_test --render-repeats`, and `pdfium_test` never types into a field.
# Not one line of `pdfrum-form` executes on either side of the comparison. An op
# that looped over the event cascade would profile real code that contributes
# exactly nothing to the gap it was built to explain — which is the failure
# `benches/src/bin/profile.rs`'s own module docs forbid.
#
# What actually distinguishes a forms document from a vector one, on the path
# both engines run, is the **annotation appearance overlay**: `pdfium_test --png`
# seeds `FPDF_ANNOT` and calls `FPDF_FFLDraw` after every bitmap render, so each
# widget's appearance stream is fetched or generated, fitted into its `/Rect` by
# `CFX_Matrix::MatchRect`, and drawn on every pass. We do the same in
# `pdfrum_doc::annot_render::overlay`, reached from the facade's `paint` under
# `RenderOptions::annotations`. That is the whole of the delta, and it is what
# `--op forms` isolates.
#
# It isolates it by **A/B rather than by direct timing**: the same warm render
# run twice, `annotations: true` against `annotations: false`, interleaved round
# by round in one process, each arm's minimum taken. A direct timing would mean
# widening a published `pdfrum-doc` signature for a profiler's convenience, and
# would measure the overlay without the page graph it appends into — which is
# not what a render pays. The difference of two minima inherits both arms' noise,
# so the split is a decomposition to be read for its *share*, and the symbol
# profile below it is the authority on where inside the overlay the time sits.
#
# `--op forms` runs its own warm loop on both arms and ignores `--warm`; there
# is no cold reading of it worth taking, because the number it explains is warm.
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
        # loop hoists the page graph and so answers the other question entirely —
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
