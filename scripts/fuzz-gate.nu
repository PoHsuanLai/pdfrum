#!/usr/bin/env nu
# The M1 fuzz gate (PLAN.md §6): "parser fuzzers running clean for 24h".
#
# Runs every parser-facing fuzz target for a share of a total time budget and
# fails on the first crash. Not part of scripts/ci.nu — that gate *compiles*
# the fuzz workspace in seconds, which is what catches a target rotting
# against an API change; this one is minutes to days of actual fuzzing.
#
# Usage:
#   scripts/fuzz-gate.nu                 # 10 minutes total, a smoke check
#   scripts/fuzz-gate.nu 86400           # the 24h M1 gate, sequential
#   scripts/fuzz-gate.nu 86400 parallel  # the 24h M1 gate, one process each
#   scripts/fuzz-gate.nu 3600 --targets [parser_load]
#
# Sequential divides the budget between the targets; parallel gives each the
# whole budget and runs them at once, so `86400 parallel` is the real 24h
# gate and costs 24 hours of wall clock rather than 24 × N.

# Every target that consumes bytes on the parser's own path to a loaded
# document. The two `object_*` targets are here because the parser is what
# feeds them in production; the two `crypt_*` and the `cmap_*` ones are here
# because a document reaches them without any other crate's help.
const DEFAULT_TARGETS = [
    parser_load parser_load_password parser_xref parser_object parser_lexer
    filters_chain filters_flate filters_lzw filters_a85 filters_ahx
    filters_rle filters_predictor
    crypt_encrypt_dict crypt_decrypt
    cmap_embedded cmap_predefined
    object_decode_text object_name_decode
]

# libFuzzer's own limits. The RSS cap is generous because `parser_load` holds
# a whole document; the per-input timeout is the point past which an input is
# a hang, which for a parser is a bug as real as a crash.
const RUNARGS = [-rss_limit_mb=4096 -timeout=25 -print_final_stats=1]

# One target, its whole log captured. Returns the target name on failure and
# null on success, so both the sequential and the parallel arm collect
# failures the same way.
def run-one [target: string, seconds: int, root: path] {
    let log = ($root | path join fuzz artifacts $"($target).log")
    let corpus = $"corpus/($target)"
    let libfuzzer_args = ($RUNARGS | append $"-max_total_time=($seconds)")
    let result = (do {
        cd ($root | path join fuzz)
        ^cargo +nightly fuzz run $target $corpus -- ...$libfuzzer_args
    } | complete)
    $"($result.stdout)($result.stderr)" | save --force $log
    if $result.exit_code == 0 {
        print $"ok   ($target)"
        return null
    }
    print $"FAIL ($target)  \(log: ($log)\)"
    open $log | lines | where {|l|
        $l =~ 'panicked|ERROR:|SUMMARY:|Test unit written'
    } | first 10 | each {|l| print $l } | ignore
    $target
}

def main [
    budget: int = 600           # total seconds of fuzzing
    mode: string = "sequential" # "sequential" or "parallel"
    --targets: list<string>     # override the default target set
] {
    let root = ($env.FILE_PWD | path dirname)
    let targets = ($targets | default $DEFAULT_TARGETS)

    if ($mode not-in [sequential parallel]) {
        print --stderr $"error: mode must be 'sequential' or 'parallel', got ($mode)"
        exit 2
    }
    if (which cargo-fuzz | is-empty) {
        print --stderr "error: cargo-fuzz not installed"
        print --stderr "       install with: cargo install cargo-fuzz --locked"
        exit 1
    }

    # `corpus/` is gitignored working state; rebuild it from the committed
    # seeds (and the oracle checkout, when present) so a fresh clone gates the
    # same way. Best-effort: a missing oracle still leaves the seeds.
    do -i { ^bash ($root | path join fuzz seed-corpus.sh) } | ignore

    let per_target = if $mode == "parallel" {
        $budget
    } else {
        [1, ($budget // ($targets | length))] | math max
    }

    print $"==> fuzz gate: ($targets | length) targets, ($mode), ($per_target)s each"
    mkdir ($root | path join fuzz artifacts)

    # `par-each` is the parallel arm, one `cargo fuzz` process per closure,
    # which is what the bash version's background jobs were. `--threads` is
    # pinned to the target count on purpose: the default pool is sized to the
    # CPU count, and on a machine with fewer cores than targets that would
    # silently serialise part of the run — turning a 24h budget into more
    # than 24h of wall clock, which is exactly what `parallel` mode exists to
    # avoid. These processes are I/O- and RSS-bound as much as CPU-bound, and
    # the M1 gate's cost is stated in wall clock.
    let failed = if $mode == "parallel" {
        $targets
        | par-each --threads ($targets | length) {|t| run-one $t $per_target $root }
        | compact
    } else {
        $targets | each {|t| run-one $t $per_target $root } | compact
    }

    print ""
    if not ($failed | is-empty) {
        print --stderr $"fuzz gate FAILED: ($failed | str join ' ')"
        print --stderr "crashing inputs are under fuzz/artifacts/<target>/; reproduce with:"
        print --stderr "  cd fuzz and cargo +nightly fuzz run <target> artifacts/<target>/<file>"
        exit 1
    }
    print $"fuzz gate clean: ($targets | length) targets, no crashes in ($budget)s."
}
