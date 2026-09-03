#!/usr/bin/env nu
# Time the PDFium oracle over the same fixtures `benches/` measures, so the
# two columns in docs/status/M8.md describe the same work on the same files.
#
# Usage: scripts/bench-oracle.nu [path-to-pdfium_test] [repeats] [rounds] [dir]
#
# The binary defaults to `$PDFRUM_ORACLE_BIN` (scripts/env.nu), which itself
# defaults to `<repo>/../pdfium-c++/out/Release/pdfium_test`.
#
# The oracle has no criterion. What it does have is `--render-repeats=<n>`,
# which renders the document n times inside one process, so the loop is
# measured and the process start, the font-database build and the file read
# are amortized rather than counted. This script runs each file twice — once
# at `n` repeats and once at 1 — and reports the *difference* divided by
# (n - 1). That difference is the marginal cost of one more render pass over
# the whole document, which is the quantity criterion's per-iteration time
# also reports, and it cancels the fixed startup cost that would otherwise
# dominate a 2 ms document.
#
# The result is deliberately not a like-for-like of `open`: `pdfium_test`
# gives no way to time loading alone, so M8.md compares render only and says
# so.

use env.nu [oracle-bin]

# `%.Nf`. Nushell rounds (`math round`) but does not pad, and these columns are
# read down rather than across, so the decimal points have to line up.
def fixed [places: int]: float -> string {
    let v = $in
    let neg = $v < 0
    let scaled = ((($v | math abs) * (10.0 ** $places)) + 0.5 | math floor | into string)
    let s = ($scaled | fill --alignment r --width ($places + 1) --character '0')
    let cut = (($s | str length) - $places)
    let out = if $places == 0 { $s } else { $"($s | str substring 0..<$cut).($s | str substring $cut..)" }
    if $neg { $"-($out)" } else { $out }
}

# `--md5` keeps the render honest — the pixels are consumed rather than
# optimized away — while writing nothing to disk, so the timing is the engine
# rather than the filesystem. Same reason the criterion side black-boxes its
# pixmaps.
def run [oracle: path, file: path, repeats: int]: nothing -> duration {
    timeit {
        # `complete` both swallows a non-zero exit (the bash `|| true`) and
        # captures both streams, so the oracle's per-page chatter stays out of
        # the table.
        ^$oracle --md5 $"--render-repeats=($repeats)" $file | complete | ignore
    }
}

# The *minimum* of several runs, not the mean. A timing sample is bounded
# below by the real cost and unbounded above by whatever else the machine was
# doing, so the smallest observation is the least contaminated estimate --
# and taking a mean of a wall-clock measurement lets one descheduled run
# inflate the answer permanently. Criterion reports a distribution because it
# takes hundreds of samples; here there are few, so the minimum is the honest
# summary.
def best-of [oracle: path, file: path, repeats: int, rounds: int]: nothing -> duration {
    1..$rounds | each { run $oracle $file $repeats } | math min
}

def main [
    oracle?: path
    repeats: int = 40
    rounds: int = 5
    # M12 moved the benchmark set from `benches/fixtures` (seven small M8 files)
    # to `benches/corpus` (44 files across six classes). Point this at
    # `fixtures` to reproduce the M8 table instead; the script is otherwise
    # unchanged, and both directories carry their own PROVENANCE.md.
    fixtures: path = benches/corpus
] {
    cd ($env.FILE_PWD | path dirname)

    let oracle = ($oracle | default (oracle-bin))
    if not ($oracle | path exists) {
        print --stderr $"error: no pdfium_test at ($oracle)"
        print --stderr "       pass its path as the first argument, or set"
        print --stderr "       PDFRUM_ORACLE_BIN / PDFRUM_ORACLE_CHECKOUT"
        exit 1
    }

    let w = [28 12 12 12]
    print ([
        ('fixture' | fill --alignment l --width $w.0)
        ('n=1 (s)' | fill --alignment r --width $w.1)
        ($"n=($repeats) \(s\)" | fill --alignment r --width $w.2)
        ('per-pass (ms)' | fill --alignment r --width $w.3)
    ] | str join ' ')
    print ($w | each {|n| '-' | fill --alignment l --width $n --character '-' } | str join ' ')

    for file in (glob $"($fixtures)/*.pdf" | sort) {
        let stem = ($file | path basename | str replace --regex '\.pdf$' '')

        # One untimed pass first: the page cache and the font database are warm
        # for both measured runs, as they are for every criterion sample after
        # its warm-up.
        run $oracle $file 1 | ignore

        let one = (best-of $oracle $file 1 $rounds) / 1sec
        let many = (best-of $oracle $file $repeats $rounds) / 1sec
        let per_pass = ($many - $one) * 1000 / ($repeats - 1)

        print ([
            ($stem | fill --alignment l --width $w.0)
            ($one | fixed 4 | fill --alignment r --width $w.1)
            ($many | fixed 4 | fill --alignment r --width $w.2)
            ($per_pass | fixed 3 | fill --alignment r --width $w.3)
        ] | str join ' ')
    }
}
