#!/usr/bin/env bash
# Peak resident set size for open+render, ours against the oracle, on the
# documents where memory is actually a question.
#
# Usage: scripts/bench-rss.sh [pdfium_test] [rounds] [dir] [tool]
#
# PLAN.md §M12's memory target is "peak RSS <= 1.5x oracle". This is what
# measures it. The time harness beside it (scripts/bench-oracle.sh) answers a
# different question with a different method, and the difference is the point:
#
# WHY MAXIMUM AND NOT MINIMUM
#
# bench-oracle.sh takes the *minimum* of several runs, because a wall-clock
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
set -euo pipefail
cd "$(dirname "$0")/.."

ORACLE=${1:-/mnt/data2/pdfium/pdfium-c++/out/Release/pdfium_test}
ROUNDS=${2:-3}
FIXTURES=${3:-benches/corpus}
TOOL=${4:-}

if [ ! -x "$ORACLE" ]; then
    echo "error: no pdfium_test at $ORACLE" >&2
    echo "       pass its path as the first argument" >&2
    exit 1
fi

# The tool defaults to the workspace target directory, which a redirected
# CARGO_TARGET_DIR moves — the same trap conformance/README.md records.
if [ -z "$TOOL" ]; then
    TARGET=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
        | tr ',' '\n' | grep -o '"target_directory":"[^"]*"' | head -1 \
        | sed 's/.*:"//;s/"$//')
    TOOL=${TARGET:-target}/release/pdfrum-tool
fi
if [ ! -x "$TOOL" ]; then
    echo "error: no pdfrum-tool at $TOOL" >&2
    echo "       build it with: cargo build --release -p pdfrum-tool" >&2
    echo "       or pass its path as the fourth argument" >&2
    exit 1
fi

TIME_BIN=/usr/bin/time
if [ ! -x "$TIME_BIN" ]; then
    echo "error: /usr/bin/time is required for ru_maxrss (the shell builtin" >&2
    echo "       'time' does not report it). Debian/Ubuntu: apt install time" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Peak RSS of one command, in kilobytes, as the kernel recorded it.
peak_kb() {
    "$TIME_BIN" -f '%M' "$@" >/dev/null 2>"$WORK/rss"
    tail -1 "$WORK/rss"
}

# The maximum over `rounds`, plus the spread, for the reason in the header.
worst_of() {
    local rounds=$1; shift
    local worst=0 best=0 t
    for _ in $(seq "$rounds"); do
        t=$(peak_kb "$@" || echo 0)
        [ -z "$t" ] && t=0
        if [ "$t" -gt "$worst" ]; then worst=$t; fi
        if [ "$best" = 0 ] || [ "$t" -lt "$best" ]; then best=$t; fi
    done
    echo "$worst $best"
}

FONT_DIR=${PDFRUM_FONT_DIR:-$(dirname "$(dirname "$ORACLE")")/../../third_party/test_fonts}
if [ ! -d "$FONT_DIR" ]; then
    # Not fatal: both engines fall back to their own default font lookup, and
    # they then differ in what they load — which is a real part of the
    # footprint, so it is said out loud rather than silently absorbed.
    echo "note: no test font directory at $FONT_DIR; both engines use their" >&2
    echo "      own defaults, which is one more difference in the ratio." >&2
    FONT_DIR=""
fi
# Both engines render to a PNG in the scratch directory rather than beside the
# corpus, so a run leaves the tree clean and both write the same way. `--md5`
# additionally keeps the pixels consumed rather than optimized away, the same
# reason scripts/bench-oracle.sh passes it.
if [ -n "$FONT_DIR" ]; then
    FONT_ARG=(--font-dir="$FONT_DIR")
else
    FONT_ARG=()
fi

# The floor: what each binary costs having done nothing. Both print "No input
# files." and exit; that is the whole runtime started and nothing else. Every
# ratio below is a ratio of totals that include this, and on a small document
# it is most of what is being compared — which is why it is measured rather
# than assumed equal.
echo "==> process floor (no document)"
read -r o_floor _ <<<"$(worst_of "$ROUNDS" "$ORACLE")"
read -r r_floor _ <<<"$(worst_of "$ROUNDS" "$TOOL")"
printf '%-32s %10s %10s %8s\n' "" "oracle KB" "pdfrum KB" "ratio"
printf '%-32s %10s %10s %8.2f\n' "(floor)" "$o_floor" "$r_floor" \
    "$(echo "scale=4; $r_floor / $o_floor" | bc)"
echo

printf '%-32s %10s %10s %8s %10s\n' document "oracle KB" "pdfrum KB" "ratio" "spread"
printf '%-32s %10s %10s %8s %10s\n' \
    "--------------------------------" "----------" "----------" "--------" "----------"

worst_ratio=0
worst_doc=""
count=0
sum_log=0
cd "$WORK"
for f in "$OLDPWD/$FIXTURES"/*.pdf; do
    [ -e "$f" ] || continue
    stem=$(basename "$f" .pdf)
    read -r o_max o_min <<<"$(worst_of "$ROUNDS" "$ORACLE" --png --md5 "${FONT_ARG[@]}" "$f")"
    read -r r_max r_min <<<"$(worst_of "$ROUNDS" "$TOOL" --png --md5 "${FONT_ARG[@]}" "$f")"
    [ "$o_max" = 0 ] && continue
    ratio=$(echo "scale=4; $r_max / $o_max" | bc)
    # The spread is reported as the larger of the two sides' max/min, so a
    # reader can tell a real 1.4x from one round's page-fault timing.
    o_spread=$(echo "scale=3; $o_max / ($o_min + 1)" | bc)
    r_spread=$(echo "scale=3; $r_max / ($r_min + 1)" | bc)
    spread=$(echo "if ($o_spread > $r_spread) $o_spread else $r_spread" | bc)
    printf '%-32s %10s %10s %8.2f %10.3f\n' "$stem" "$o_max" "$r_max" "$ratio" "$spread"
    if [ "$(echo "$ratio > $worst_ratio" | bc)" = 1 ]; then
        worst_ratio=$ratio
        worst_doc=$stem
    fi
    # A geometric mean, because a ratio's average is its logarithm's: an
    # arithmetic mean of ratios is dominated by whichever document happens to
    # be furthest above 1 and is not the reciprocal of the reverse comparison.
    sum_log=$(echo "scale=8; $sum_log + l($ratio)" | bc -l)
    count=$((count + 1))
done
cd "$OLDPWD"

echo
if [ "$count" -gt 0 ]; then
    geo=$(echo "scale=4; e($sum_log / $count)" | bc -l)
    printf 'geometric mean ratio over %d documents: %.2fx\n' "$count" "$geo"
    printf 'worst document: %s at %.2fx\n' "$worst_doc" "$worst_ratio"
    echo
    echo "PLAN.md §M12 target: peak RSS <= 1.5x oracle."
    if [ "$(echo "$worst_ratio <= 1.5" | bc)" = 1 ]; then
        echo "MET on every document."
    elif [ "$(echo "$geo <= 1.5" | bc)" = 1 ]; then
        echo "MET on the geometric mean; $worst_doc exceeds it. See docs/status/M12.md."
    else
        echo "NOT MET. See docs/status/M12.md."
    fi
else
    echo "no documents measured; is $FIXTURES populated?"
fi
