#!/usr/bin/env bash
# Time the PDFium oracle over the same fixtures `benches/` measures, so the
# two columns in docs/status/M8.md describe the same work on the same files.
#
# Usage: scripts/bench-oracle.sh [path-to-pdfium_test] [repeats] [rounds]
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
set -euo pipefail
cd "$(dirname "$0")/.."

ORACLE=${1:-/mnt/data2/pdfium/pdfium-c++/out/Release/pdfium_test}
REPEATS=${2:-40}
ROUNDS=${3:-5}
# M12 moved the benchmark set from `benches/fixtures` (seven small M8 files) to
# `benches/corpus` (44 files across six classes). Point this at `fixtures` to
# reproduce the M8 table instead; the script is otherwise unchanged, and both
# directories carry their own PROVENANCE.md.
FIXTURES=${4:-benches/corpus}

if [ ! -x "$ORACLE" ]; then
    echo "error: no pdfium_test at $ORACLE" >&2
    echo "       pass its path as the first argument" >&2
    exit 1
fi

# `--md5` keeps the render honest — the pixels are consumed rather than
# optimized away — while writing nothing to disk, so the timing is the engine
# rather than the filesystem. Same reason the criterion side black-boxes its
# pixmaps.
run() {
    local file=$1 repeats=$2
    local start end
    start=$(date +%s.%N)
    "$ORACLE" --md5 "--render-repeats=$repeats" "$file" >/dev/null 2>&1 || true
    end=$(date +%s.%N)
    echo "$end - $start" | bc
}

# The *minimum* of several runs, not the mean. A timing sample is bounded
# below by the real cost and unbounded above by whatever else the machine was
# doing, so the smallest observation is the least contaminated estimate --
# and taking a mean of a wall-clock measurement lets one descheduled run
# inflate the answer permanently. Criterion reports a distribution because it
# takes hundreds of samples; here there are few, so the minimum is the honest
# summary.
best_of() {
    local file=$1 repeats=$2 rounds=$3
    local best='' t
    for _ in $(seq "$rounds"); do
        t=$(run "$file" "$repeats")
        if [ -z "$best" ] || [ "$(echo "$t < $best" | bc)" = 1 ]; then
            best=$t
        fi
    done
    echo "$best"
}

printf '%-28s %12s %12s %12s\n' fixture "n=1 (s)" "n=$REPEATS (s)" "per-pass (ms)"
printf '%-28s %12s %12s %12s\n' ---------------------------- ------------ ------------ ------------

for file in "$FIXTURES"/*.pdf; do
    [ -e "$file" ] || continue
    stem=$(basename "$file" .pdf)

    # One untimed pass first: the page cache and the font database are warm
    # for both measured runs, as they are for every criterion sample after
    # its warm-up.
    run "$file" 1 >/dev/null

    one=$(best_of "$file" 1 "$ROUNDS")
    many=$(best_of "$file" "$REPEATS" "$ROUNDS")
    per_pass=$(echo "scale=6; ($many - $one) * 1000 / ($REPEATS - 1)" | bc)

    printf '%-28s %12.4f %12.4f %12.3f\n' "$stem" "$one" "$many" "$per_pass"
done
