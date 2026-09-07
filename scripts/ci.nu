#!/usr/bin/env nu
# CI gate: fmt, clippy -D warnings, nextest, doctests, cargo-deny (if
# installed), and the pure-Rust dependency-tree check.

def main [] {
    cd ($env.FILE_PWD | path dirname)

    print "==> cargo fmt --check"
    ^cargo fmt --all -- --check

    print "==> cargo clippy (deny warnings)"
    ^cargo clippy --workspace --all-targets -- -D warnings

    print "==> cargo nextest run (with the tool's and the CLI's javascript features and pdfrum/svg)"
    ^cargo nextest run --workspace --features pdfrum-tool/javascript,pdfrum-cli/javascript,pdfrum/svg

    print "==> cargo test --doc (nextest silently skips doctests)"
    ^cargo test --doc --workspace --features pdfrum/svg

    print "==> cargo doc --no-deps (deny warnings)"
    with-env { RUSTDOCFLAGS: '-D warnings' } {
        ^cargo doc --no-deps --workspace
    }

    ^./scripts/check-rustdoc-coverage.nu

    ^./scripts/api-snapshot.nu check

    print "==> cargo deny check"
    if (which cargo-deny | is-empty) {
        print --stderr "warning: cargo-deny not installed; skipping license/ban audit"
        print --stderr "         install with: cargo install cargo-deny --locked"
    } else {
        ^cargo deny check
    }

    print "==> pure-Rust check (no -sys / cc / cmake / pkg-config / bindgen)"
    # GPU tree dlopen shims, by name. `linux-raw-sys` is rustix constants.
    # `js-sys` is wasm-bindgen's JS stdlib.
    let gpu_sys_exemptions = [renderdoc-sys wayland-sys]
    let pure_rust_sys = [linux-raw-sys js-sys]
    let native_build_crates = [cc cmake pkg-config bindgen]

    let forbidden = (^cargo tree -e normal --workspace --prefix none
        | lines | split column ' ' name | get name | uniq
        | where {|c| ($c | str ends-with '-sys') or ($c in $native_build_crates) }
        | where {|c| $c not-in $gpu_sys_exemptions and $c not-in $pure_rust_sys }
        | sort)
    if not ($forbidden | is-empty) {
        print --stderr "error: forbidden native-build crates in the dependency tree:"
        $forbidden | each {|c| print --stderr $"  ($c)" } | ignore
        exit 1
    }
    print "ok: dependency tree is pure Rust"

    print "==> the C header (generated, committed, and matching the library)"
    if (which cbindgen | is-empty) {
        print --stderr "warning: cbindgen not installed; skipping the C header check"
        print --stderr "         install with: cargo install cbindgen --locked"
    } else if (which cc | is-empty) {
        print --stderr "warning: no C compiler, so no libpdfrum.so was built;"
        print --stderr "         skipping the C header check"
    } else {
        ^./crates/pdfrum-capi/ctest/run.sh --build-only
        ^./scripts/capi-header.nu check
    }

    print "==> the C test (libpdfrum)"
    if (which cc | is-empty) {
        print --stderr "warning: no C compiler (cc); skipping the C test"
        print --stderr "         it is a test-time tool, not a build dependency —"
        print --stderr "         see CONTRIBUTING.md"
    } else {
        ^./crates/pdfrum-capi/ctest/run.sh
    }

    # Must `cd` into the crate: cargo reads `.cargo/config.toml` relative to
    # the invocation directory, not the manifest.
    print "==> the WebAssembly tests (Node)"
    let wasm_target = (^rustup target list --installed | lines | any {|t| $t == "wasm32-unknown-unknown" })
    if (which node | is-empty) {
        print --stderr "warning: node not installed; skipping the WebAssembly tests"
    } else if not $wasm_target {
        print --stderr "warning: the wasm32-unknown-unknown target is not installed;"
        print --stderr "         skipping the WebAssembly tests"
        print --stderr "         install with: rustup target add wasm32-unknown-unknown"
    } else if (which wasm-bindgen-test-runner | is-empty) {
        print --stderr "warning: wasm-bindgen-test-runner not installed; skipping the WebAssembly tests"
        print --stderr "         install with: cargo install wasm-bindgen-cli --locked"
    } else {
        cd crates/pdfrum-wasm
        ^cargo test --target wasm32-unknown-unknown
        cd ../..
    }

    # fuzz/ is its own workspace (libfuzzer-sys / cc). Do not add it to
    # members. This only typechecks; running is scripts/fuzz-gate.nu.
    print "==> cargo check (fuzz workspace)"
    ^cargo check --manifest-path fuzz/Cargo.toml --all-targets

    print ""
    print "CI green."
}
