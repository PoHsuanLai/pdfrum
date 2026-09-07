#!/usr/bin/env nu
# Builds `crates/pdfrum-wasm` into the npm package `crates/pdfrum-wasm/pkg`,
# and holds the module to a size budget.
#
# Three tools in a row, which is what `wasm-pack` would have wrapped:
#
#   1. `cargo build --profile wasm --target wasm32-unknown-unknown`
#   2. `wasm-bindgen --target web`, which emits the loader, the `.d.ts` and the
#      `_bg.wasm` the loader instantiates
#   3. `wasm-opt -Oz`, which is where roughly 5% of the module goes
#
# **Why not `wasm-pack`.** It was tried first and cannot drive this crate: it
# runs `wasm-opt` itself with a hardcoded `-O` and no feature flags, and that
# invocation *fails* here — Rust's `wasm32-unknown-unknown` emits bulk-memory
# (`memory.copy`, `memory.fill`) and non-trapping float-to-int, and `wasm-opt`
# rejects both unless told they are allowed. The flags cannot be supplied
# either: wasm-pack reads `[package.metadata.wasm-pack.profile.<name>]` only
# for `dev`, `release` and `profiling`, and this crate builds under the
# workspace's `wasm` profile, under which it consults no metadata at all.
# Running the three tools directly costs this file and buys the before/after
# measurement the budget check needs.
#
# `pkg/` is git-ignored: it is a build output, and the thing under review is
# this script plus the Rust that feeds it.
#
# Requires `wasm-bindgen` and `wasm-opt` on PATH, and the `wasm32-unknown-unknown`
# target. All three are tools, not dependencies:
#
#   cargo install wasm-bindgen-cli --locked   # version must match Cargo.toml
#   cargo install wasm-opt --locked
#   rustup target add wasm32-unknown-unknown

# The largest the optimized module may be, in bytes.
#
# Measured 2026-09-05 at 4,827,012 bytes (4.60 MiB) and set 20% above that, so
# ordinary growth in the facade does not fail the gate and a step change does.
# Raising it is a deliberate commit with a sentence about what got bigger.
const BUDGET = 5792414

# The features `wasm-opt`'s validator must be told to allow.
#
# Not opt-ins to anything experimental: they are what rustc already emitted
# into the module, and every engine that runs WebAssembly today supports them.
# They are "features" only in `wasm-opt`'s own vocabulary, which still defaults
# to the 2017 MVP.
const WASM_OPT_FEATURES = [
    "--enable-bulk-memory"
    "--enable-nontrapping-float-to-int"
    "--enable-sign-ext"
    "--enable-mutable-globals"
    "--enable-multivalue"
    "--enable-reference-types"
]

# Builds the package and checks the budget.
def main [
    --skip-opt  # Skip `wasm-opt`, for a machine that does not have it. The
                # budget is then checked against the unoptimized module, which
                # is strictly harder to pass, so a pass still means something.
] {
    let root = ($env.FILE_PWD | path dirname)
    let crate = ($root | path join "crates" "pdfrum-wasm")
    let pkg = ($crate | path join "pkg")

    print "==> cargo build (wasm32-unknown-unknown, profile wasm)"
    ^cargo build --manifest-path ($crate | path join "Cargo.toml") --profile wasm --target wasm32-unknown-unknown --lib

    let target = (if ($env.CARGO_TARGET_DIR? | is-empty) {
        $root | path join "target"
    } else {
        $env.CARGO_TARGET_DIR
    })
    let module = ($target | path join "wasm32-unknown-unknown" "wasm" "pdfrum_wasm.wasm")

    print "==> wasm-bindgen (target web)"
    rm -rf $pkg
    ^wasm-bindgen --target web --out-dir $pkg --out-name pdfrum $module

    let built = ($pkg | path join "pdfrum_bg.wasm")
    let before = (ls $built | get 0.size | into int)
    print $"    module before wasm-opt: ($before) bytes"

    let final_size = if $skip_opt {
        print "    wasm-opt skipped (--skip-opt)"
        $before
    } else {
        print "==> wasm-opt -Oz"
        let optimized = ($pkg | path join "pdfrum_bg.opt.wasm")
        ^wasm-opt -Oz ...$WASM_OPT_FEATURES -o $optimized $built
        let after = (ls $optimized | get 0.size | into int)
        mv --force $optimized $built
        print $"    module after wasm-opt:  ($after) bytes"
        print $"    saved: ($before - $after) bytes"
        $after
    }

    # The package manifest, written here rather than committed: its `files`
    # list names the artefacts this script just produced, so a hand-edited copy
    # could name a file that is no longer built.
    let manifest = {
        name: "pdfrum"
        version: (open ($root | path join "Cargo.toml") | get workspace.package.version)
        description: "A pure-Rust PDF engine for the web: open, render, extract and fill PDFs"
        license: "MIT OR Apache-2.0"
        repository: (open ($root | path join "Cargo.toml") | get workspace.package.repository)
        type: "module"
        main: "pdfrum.js"
        types: "pdfrum.d.ts"
        sideEffects: ["./snippets/*"]
        files: ["pdfrum_bg.wasm" "pdfrum.js" "pdfrum.d.ts" "README.md"]
    }
    $manifest | to json --indent 2 | save --force ($pkg | path join "package.json")
    cp ($crate | path join "README.md") ($pkg | path join "README.md")

    print ""
    print $"==> size budget: ($final_size) of ($BUDGET) bytes"
    if $final_size > $BUDGET {
        print --stderr $"error: the module is ($final_size) bytes, over the ($BUDGET)-byte budget"
        print --stderr "       Either find what grew, or raise BUDGET in this script in a"
        print --stderr "       commit that says what got bigger and why it is worth it."
        exit 1
    }
    let headroom = (100.0 - ($final_size * 100.0 / $BUDGET))
    print $"ok: ($headroom | math round --precision 1)% under budget"
    print $"package: ($pkg)"
}
