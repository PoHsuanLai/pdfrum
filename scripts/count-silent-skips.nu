#!/usr/bin/env nu
# Fail if a new test can pass by returning early when an input is absent.
#
# GPU tests and oracle-gated tests skip with `else { return }` rather than
# `#[ignore]`, so nextest counts them as passes. The GPU job on Linux runs
# the suite for real (lavapipe). This script is the source-level ratchet:
# a new skip site has to bump the pin below, so it cannot join silently.

const PINNED = 33

def main [] {
    let root = ($env.FILE_PWD | path dirname)
    let hits = (^rg --glob '**/tests/*.rs' --count-matches 'else \{ return' $root
        | complete)
    if $hits.exit_code == 1 and ($hits.stdout | str trim | is-empty) {
        print --stderr "error: silent-skip search matched nothing; is rg installed?"
        exit 1
    }
    if $hits.exit_code not-in [0 1] {
        print --stderr $hits.stderr
        exit $hits.exit_code
    }
    let total = ($hits.stdout
        | lines
        | where {|l| $l != ""}
        | each {|l| $l | split row ':' | last | into int }
        | math sum)
    if $total != $PINNED {
        print --stderr $"error: silent-skip sites are ($total), pin is ($PINNED)"
        print --stderr "       GPU and oracle tests skip with `else { return }`,"
        print --stderr "       which nextest counts as a pass. Bump PINNED in"
        print --stderr "       scripts/count-silent-skips.nu only with a reason."
        print --stderr $hits.stdout
        exit 1
    }
    print $"silent-skip floor: ($total) sites, pinned"
}
