#!/usr/bin/env nu
# Peak resident set size for open+render, ours against the oracle, on the
# documents where memory is actually a question.
#
# Usage: scripts/bench-rss.nu [pdfium_test] [rounds] [dir] [tool]
#
# The binary defaults to `$PDFRUM_ORACLE_BIN` (scripts/env.nu), which itself
# defaults to `<repo>/../pdfium-c++/out/Release/pdfium_test`.
#
# The memory target is "peak RSS <= 1.5x oracle". This is what
# measures it. The time harness beside it (scripts/bench-oracle.nu) answers a
# different question with a different method, and the difference is the point:
#
# WHY MAXIMUM AND NOT MINIMUM
#
# bench-oracle.nu takes the *minimum* of several runs, because a wall-clock
# sample is bounded below by the real cost and inflated by whatever else the
# machine was doing. Peak RSS is the opposite quantity: it is a high-water
# mark the kernel records, not a rate, and it is bounded *above* by what the
# process genuinely touched. Contention does not inflate it — another process
# competing for CPU does not make ours allocate — so the run-to-run spread is
# small and its source is allocator and page-fault timing rather than noise.
# We therefore report the maximum over `rounds`, which is the true worst case
# a caller has to provision for, and print the spread so a reader can see when
# it is not tight.
#
# WHY ONE RENDER PASS AND NOT MANY
#
# `--render-repeats=n` is how the time harness amortizes process start. Here
# it would *hide* the thing being measured: a cache that grows without bound
# shows up on the second pass and later, and a peak taken over forty passes
# would charge every engine with its steady state rather than its worst
# moment. Both sides therefore render each document exactly once, which is
# also what a caller rendering a document does.
#
# WHY ru_maxrss AND NOT /proc SAMPLING
#
# `/usr/bin/time -v` reports the kernel's own `ru_maxrss` for the process and
# its waited-for children: an exact high-water mark, not a sample. A sampler
# polling /proc/<pid>/status races the peak and reports whatever it happened
# to catch, which on a document that allocates 100 MB for forty milliseconds
# is usually nothing. The tradeoff is that `ru_maxrss` cannot say *when* the
# peak happened or what held it — that is a profiler's question, and this is
# a budget check.
#
# WHAT THE NUMBER INCLUDES, ON BOTH SIDES
#
# The whole process: binary, runtime, font database, the parsed document and
# every buffer the render touched. That flatters neither engine and is the
# quantity a caller cares about, but it means a *ratio* near 1.0 on a small
# document is mostly comparing two binaries' baseline footprint rather than
# two renderers. The empty-process floor is measured and printed first for
# exactly that reason; read the ratios on the heavy documents.

use env.nu [oracle-bin]

# `%.Nf`. Nushell rounds but does not pad, and these columns are read down.
def fixed [places: int]: float -> string {
    let v = $in
    let neg = $v < 0
    let scaled = ((($v | math abs) * (10.0 ** $places)) + 0.5 | math floor | into string)
    let s = ($scaled | fill --alignment r --width ($places + 1) --character '0')
    let cut = (($s | str length) - $places)
    let out = if $places == 0 { $s } else { $"($s | str substring 0..<$cut).($s | str substring $cut..)" }
    if $neg { $"-($out)" } else { $out }
}

# Peak RSS of one command, in kilobytes, as the kernel recorded it.
def peak-kb [time_bin: path, argv: list<string>]: nothing -> int {
    let r = (do --ignore-errors { ^$time_bin -f '%M' ...$argv } | complete)
    # `%M` is the last line of `time`'s stderr; a non-zero exit adds a
    # "Command exited with non-zero status" line above it, which is why the
    # last line is taken rather than the whole stream.
    $r.stderr | lines | where {|l| $l =~ '^[0-9]+$' } | last 1 | get -o 0 | default '0' | into int
}

# The maximum over `rounds`, plus the spread, for the reason in the header.
def worst-of [time_bin: path, rounds: int, argv: list<string>]: nothing -> record<max: int, min: int> {
    let samples = (1..$rounds | each { peak-kb $time_bin $argv })
    { max: ($samples | math max), min: ($samples | math min) }
}

def main [
    oracle?: path
    rounds: int = 3
    fixtures: path = benches/corpus
    tool?: path
] {
    cd ($env.FILE_PWD | path dirname)
    let root = (pwd)

    let oracle = ($oracle | default (oracle-bin))
    if not ($oracle | path exists) {
        print --stderr $"error: no pdfium_test at ($oracle)"
        print --stderr "       pass its path as the first argument, or set"
        print --stderr "       PDFRUM_ORACLE_BIN / PDFRUM_ORACLE_CHECKOUT"
        exit 1
    }

    # The tool defaults to the workspace target directory, which a redirected
    # CARGO_TARGET_DIR moves — the same trap conformance/README.md records.
    let tool = if $tool == null {
        let target = (do --ignore-errors {
            ^cargo metadata --format-version 1 --no-deps | from json | get target_directory
        })
        ([($target | default 'target') release pdfrum-tool] | path join)
    } else { $tool }
    if not ($tool | path exists) {
        print --stderr $"error: no pdfrum-tool at ($tool)"
        print --stderr "       build it with: cargo build --release -p pdfrum-tool"
        print --stderr "       or pass its path as the fourth argument"
        exit 1
    }

    # Quoted: a bare path in expression position is a command invocation.
    let time_bin = '/usr/bin/time'
    if not ($time_bin | path exists) {
        print --stderr "error: /usr/bin/time is required for ru_maxrss (the shell builtin"
        print --stderr "       'time' does not report it). Debian/Ubuntu: apt install time"
        exit 1
    }

    let work = (mktemp --directory)

    let font_dir = ($env.PDFRUM_FONT_DIR?
        | default ([($oracle | path dirname | path dirname) .. .. third_party test_fonts] | path join))
    let font_arg = if ($font_dir | path exists) {
        [$"--font-dir=($font_dir)"]
    } else {
        # Not fatal: both engines fall back to their own default font lookup,
        # and they then differ in what they load — which is a real part of the
        # footprint, so it is said out loud rather than silently absorbed.
        print --stderr $"note: no test font directory at ($font_dir); both engines use their"
        print --stderr "      own defaults, which is one more difference in the ratio."
        []
    }
    # Both engines render to a PNG in the scratch directory rather than beside
    # the corpus, so a run leaves the tree clean and both write the same way.
    # `--md5` additionally keeps the pixels consumed rather than optimized
    # away, the same reason scripts/bench-oracle.nu passes it.
    #
    # The scratch directory is reached by *copying each document into it* and
    # rendering the copy, not by `cd`-ing there first. `pdfium_test --png`
    # writes `<pdf-path>.<page>.png` next to its **input**, so the working
    # directory does not move the output — which the bash predecessor assumed
    # it did, and which is why a run of it left a dozen untracked PNGs in
    # `benches/corpus/`. Found and fixed during the nushell migration,
    # 2026-09-02.

    # The floor: what each binary costs having done nothing. Both print "No
    # input files." and exit; that is the whole runtime started and nothing
    # else. Every ratio below is a ratio of totals that include this, and on a
    # small document it is most of what is being compared — which is why it is
    # measured rather than assumed equal.
    print "==> process floor (no document)"
    let o_floor = (worst-of $time_bin $rounds [$oracle]).max
    let r_floor = (worst-of $time_bin $rounds [$tool]).max
    print ([('' | fill -a l -w 32) ('oracle KB' | fill -a r -w 10) ('pdfrum KB' | fill -a r -w 10) ('ratio' | fill -a r -w 8)] | str join ' ')
    print ([
        ('(floor)' | fill -a l -w 32)
        ($o_floor | into string | fill -a r -w 10)
        ($r_floor | into string | fill -a r -w 10)
        (($r_floor / $o_floor) | fixed 2 | fill -a r -w 8)
    ] | str join ' ')
    print ""

    print ([('document' | fill -a l -w 32) ('oracle KB' | fill -a r -w 10) ('pdfrum KB' | fill -a r -w 10) ('ratio' | fill -a r -w 8) ('spread' | fill -a r -w 10)] | str join ' ')
    print ([32 10 10 8 10] | each {|n| '-' | fill -a l -w $n -c '-' } | str join ' ')

    # Every document becomes a row of a table: the two peaks, their ratio, and
    # the spread. The summary below is then `where`/`math` over that table
    # rather than four accumulators carried through the loop.
    let rows = (glob ([$root $fixtures '*.pdf'] | path join) | sort | each {|f|
        let stem = ($f | path basename | str replace --regex '\.pdf$' '')
        let scratch = ([$work ($f | path basename)] | path join)
        cp $f $scratch
        let o = (worst-of $time_bin $rounds ([$oracle --png --md5] ++ $font_arg ++ [$scratch]))
        let r = (worst-of $time_bin $rounds ([$tool --png --md5] ++ $font_arg ++ [$scratch]))
        if $o.max == 0 { return null }
        {
            document: $stem
            oracle: $o.max
            pdfrum: $r.max
            ratio: ($r.max / $o.max)
            # The spread is reported as the larger of the two sides' max/min,
            # so a reader can tell a real 1.4x from one round's page-fault
            # timing.
            spread: ([($o.max / ($o.min + 1)) ($r.max / ($r.min + 1))] | math max)
        }
    } | compact)

    for row in $rows {
        print ([
            ($row.document | fill -a l -w 32)
            ($row.oracle | into string | fill -a r -w 10)
            ($row.pdfrum | into string | fill -a r -w 10)
            ($row.ratio | fixed 2 | fill -a r -w 8)
            ($row.spread | fixed 3 | fill -a r -w 10)
        ] | str join ' ')
    }

    print ""
    if ($rows | is-empty) {
        print $"no documents measured; is ($fixtures) populated?"
        return
    }

    # A geometric mean, because a ratio's average is its logarithm's: an
    # arithmetic mean of ratios is dominated by whichever document happens to
    # be furthest above 1 and is not the reciprocal of the reverse comparison.
    let geo = ($rows | get ratio | each {|r| $r | math ln } | math avg | math exp)
    let worst = ($rows | sort-by ratio | last)

    print $"geometric mean ratio over ($rows | length) documents: ($geo | fixed 2)x"
    print $"worst document: ($worst.document) at ($worst.ratio | fixed 2)x"
    print ""
    print "Target: peak RSS <= 1.5x oracle."
    if $worst.ratio <= 1.5 {
        print "MET on every document."
    } else if $geo <= 1.5 {
        print $"MET on the geometric mean; ($worst.document) exceeds it."
    } else {
        print "NOT MET."
    }

    rm --recursive --force $work
}
