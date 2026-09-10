#!/usr/bin/env nu
# Fail if a new test can pass by returning early when an input is absent.
#
# GPU tests and oracle-gated tests skip with `else { return }` rather than
# `#[ignore]`, so nextest counts them as passes. The GPU job on Linux runs
# the suite for real (lavapipe). This script is the source-level ratchet:
# a new skip site has to bump the pin below, so it cannot join silently.
#
# Walks `crates/*/tests/*.rs` in nushell so the gate does not need `rg`.

# 33 plus six GPU tests for roundtrip counters, texture pooling, present
# without readback, isolated-group layers, snapshot_rect, and masked layers.
# Same skip as the rest of `pdfrum-raster-vello/tests/gpu.rs`: no adapter
# returns, and the Linux GPU job runs them on lavapipe.
const PINNED = 39
const NEEDLE = 'else { return'

def main [] {
    let root = ($env.FILE_PWD | path dirname)
    cd $root
    let files = (glob 'crates/*/tests/*.rs' | sort)
    if ($files | is-empty) {
        print --stderr "error: silent-skip search matched no test files"
        exit 1
    }
    let hits = (
        $files
        | each {|path|
            {
                path: $path
                n: ((open --raw $path | split row $NEEDLE | length) - 1)
            }
        }
        | where {|h| $h.n > 0 }
    )
    let total = (if ($hits | is-empty) { 0 } else { $hits | get n | math sum })
    if $total != $PINNED {
        print --stderr $"error: silent-skip sites are ($total), pin is ($PINNED)"
        print --stderr "       GPU and oracle tests skip with `else { return }`,"
        print --stderr "       which nextest counts as a pass. Bump PINNED in"
        print --stderr "       scripts/count-silent-skips.nu only with a reason."
        $hits | each {|h| print --stderr $"($h.path):($h.n)" } | ignore
        exit 1
    }
    print $"silent-skip floor: ($total) sites, pinned"
}
