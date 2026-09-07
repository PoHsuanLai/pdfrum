#!/usr/bin/env nu
# Builds `crates/pdfrum-wasm` into `crates/pdfrum-wasm/pkg` and holds the
# module to a size budget.
#
# Not wasm-pack: rustc emits bulk-memory / non-trapping float-to-int, and
# wasm-pack's wasm-opt cannot take those flags (it only reads metadata for
# `dev`/`release`/`profiling`, not the workspace `wasm` profile).
#
#   ./scripts/wasm-package.nu            # cargo + wasm-bindgen + wasm-opt -Oz
#   ./scripts/wasm-package.nu --skip-opt # skip wasm-opt; budget is then harder
#
# Requires:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --locked   # version must match Cargo.toml
#   cargo install wasm-opt --locked

# 20% above the measured shipped module. Raising it is a commit that says
# what got bigger.
const BUDGET = 5792414

# What rustc already emitted. wasm-opt still defaults to the 2017 MVP.
const WASM_OPT_FEATURES = [
    "--enable-bulk-memory"
    "--enable-nontrapping-float-to-int"
    "--enable-sign-ext"
    "--enable-mutable-globals"
    "--enable-multivalue"
    "--enable-reference-types"
]

def main [
    --skip-opt  # skip wasm-opt; the budget is then checked on the unoptimized module
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

    # Written here: `files` names what this script just produced.
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
        print --stderr "       find what grew, or raise BUDGET in a commit that says why"
        exit 1
    }
    let headroom = (100.0 - ($final_size * 100.0 / $BUDGET))
    print $"ok: ($headroom | math round --precision 1)% under budget"
    print $"package: ($pkg)"
}
