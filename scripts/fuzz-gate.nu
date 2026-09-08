#!/usr/bin/env nu
# Run the parser-facing fuzz targets for a time budget. Not part of
# scripts/ci.nu — that gate only typechecks fuzz/.
#
#   scripts/fuzz-gate.nu                 # 10 min total, sequential
#   scripts/fuzz-gate.nu 86400           # 24 h sequential
#   scripts/fuzz-gate.nu 86400 parallel  # 24 h, one process per target
#   scripts/fuzz-gate.nu 3600 --targets [parser_load]
#
# Sequential divides the budget; parallel gives each the whole budget.
# page_*, text_*, edit_* exist; run them with --targets.

const DEFAULT_TARGETS = [
    parser_load parser_load_password parser_xref parser_object parser_lexer
    filters_chain filters_flate filters_lzw filters_a85 filters_ahx
    filters_rle filters_predictor filters_ccitt
    crypt_encrypt_dict crypt_decrypt
    cmap_embedded cmap_predefined
    object_decode_text object_name_decode
]

# RSS cap for a whole document; hang timeout is a parser bug.
const RUNARGS = [-rss_limit_mb=4096 -timeout=25 -print_final_stats=1]

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
    budget: int = 600           # total seconds
    mode: string = "sequential" # sequential | parallel
    --targets: list<string>     # override DEFAULT_TARGETS
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

    # corpus/ is gitignored; rebuild from seeds/ (and the oracle, when present).
    do -i { ^bash ($root | path join fuzz seed-corpus.sh) } | ignore

    let per_target = if $mode == "parallel" {
        $budget
    } else {
        [1, ($budget // ($targets | length))] | math max
    }

    print $"==> fuzz gate: ($targets | length) targets, ($mode), ($per_target)s each"
    mkdir ($root | path join fuzz artifacts)

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
        print --stderr "  cd fuzz && cargo +nightly fuzz run <target> artifacts/<target>/<file>"
        exit 1
    }
    print $"fuzz gate clean: ($targets | length) targets, no crashes in ($budget)s."
}
