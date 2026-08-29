#!/usr/bin/env bash
# The M1 fuzz gate (PLAN.md §6): "parser fuzzers running clean for 24h".
#
# Runs every parser-facing fuzz target for a share of a total time budget and
# fails on the first crash. Not part of scripts/ci.sh — this is minutes to
# days of work, where the fast gate is seconds.
#
# Usage:
#   scripts/fuzz-gate.sh                 # 10 minutes total, a smoke check
#   scripts/fuzz-gate.sh 86400           # the 24h M1 gate, sequential
#   scripts/fuzz-gate.sh 86400 parallel  # the 24h M1 gate, one process each
#   FUZZ_TARGETS="parser_load" scripts/fuzz-gate.sh 3600
#
# Sequential divides the budget between the targets; parallel gives each the
# whole budget and runs them at once, so `parallel 86400` is the real 24h
# gate and costs 24 hours of wall clock rather than 24 × N.
set -uo pipefail
cd "$(dirname "$0")/.."

BUDGET="${1:-600}"
MODE="${2:-sequential}"

# Every target that consumes bytes on the parser's own path to a loaded
# document. The two `object_*` targets are here because the parser is what
# feeds them in production; the two `crypt_*` and the `cmap_*` ones are here
# because a document reaches them without any other crate's help.
DEFAULT_TARGETS="
parser_load
parser_load_password
parser_xref
parser_object
parser_lexer
filters_chain
filters_flate
filters_lzw
filters_a85
filters_ahx
filters_rle
filters_predictor
crypt_encrypt_dict
crypt_decrypt
cmap_embedded
cmap_predefined
object_decode_text
object_name_decode
"
read -r -a TARGETS <<<"$(echo "${FUZZ_TARGETS:-$DEFAULT_TARGETS}" | tr '\n' ' ')"

if ! command -v cargo-fuzz >/dev/null 2>&1; then
    echo "error: cargo-fuzz not installed" >&2
    echo "       install with: cargo install cargo-fuzz --locked" >&2
    exit 1
fi

# `corpus/` is gitignored working state; rebuild it from the committed seeds
# (and the oracle checkout, when present) so a fresh clone gates the same way.
bash fuzz/seed-corpus.sh >/dev/null 2>&1 || true

# libFuzzer's own limits. The RSS cap is generous because `parser_load` holds
# a whole document; the per-input timeout is the point past which an input is
# a hang, which for a parser is a bug as real as a crash.
RUNARGS=(-rss_limit_mb=4096 -timeout=25 -print_final_stats=1)

if [ "$MODE" = "parallel" ]; then
    per_target="$BUDGET"
else
    per_target=$((BUDGET / ${#TARGETS[@]}))
    [ "$per_target" -lt 1 ] && per_target=1
fi

echo "==> fuzz gate: ${#TARGETS[@]} targets, $MODE, ${per_target}s each"
mkdir -p fuzz/artifacts

run_one() {
    local t="$1"
    local log="fuzz/artifacts/$t.log"
    if (cd fuzz && cargo +nightly fuzz run "$t" "corpus/$t" \
            -- "${RUNARGS[@]}" "-max_total_time=$per_target") >"$log" 2>&1; then
        echo "ok   $t"
        return 0
    fi
    echo "FAIL $t  (log: $log)"
    grep -E 'panicked|ERROR:|SUMMARY:|Test unit written' "$log" | head -10
    return 1
}

failed=()
if [ "$MODE" = "parallel" ]; then
    pids=()
    for t in "${TARGETS[@]}"; do
        run_one "$t" &
        pids+=("$!:$t")
    done
    for entry in "${pids[@]}"; do
        wait "${entry%%:*}" || failed+=("${entry#*:}")
    done
else
    for t in "${TARGETS[@]}"; do
        run_one "$t" || failed+=("$t")
    done
fi

echo
if [ ${#failed[@]} -ne 0 ]; then
    echo "fuzz gate FAILED: ${failed[*]}" >&2
    echo "crashing inputs are under fuzz/artifacts/<target>/; reproduce with:" >&2
    echo "  cd fuzz && cargo +nightly fuzz run <target> artifacts/<target>/<file>" >&2
    exit 1
fi
echo "fuzz gate clean: ${#TARGETS[@]} targets, no crashes in ${BUDGET}s."
