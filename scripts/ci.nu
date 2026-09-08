#!/usr/bin/env nu
# CI gate: fmt, clippy -D warnings, pdfrum feature combinations, nextest,
# doctests, cargo-deny (if installed), and the pure-Rust dependency-tree
# check.

def main [] {
    cd ($env.FILE_PWD | path dirname)

    print "==> cargo fmt --check"
    ^cargo fmt --all -- --check

    print "==> cargo clippy (deny warnings)"
    ^cargo clippy --workspace --all-targets --locked -- -D warnings

    # Default features leave whole subsystems unlinted — `pdfrum-form`'s boa
    # binding (the code that runs a document's own JavaScript), the facade's
    # SVG ingestion, and `pdfrum-render`'s png. The workspace's
    # `unwrap_used`/`expect_used`/`panic` denials are the guarantee
    # SECURITY.md rests on, so they have to reach that code too.
    print "==> cargo clippy --all-features (deny warnings)"
    ^cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

    # Default features are clippy and nextest. This is the rest: headless,
    # each default-off flag, the advertised named-backend build, and
    # `--all-features`. `--lib` so integration tests that assume a rasterizer
    # do not have to declare `required-features`.
    print "==> pdfrum feature combinations"
    let combos = [
        {name: 'no-default', flags: [--no-default-features]}
        {name: 'no-default+forms', flags: [--no-default-features --features forms]}
        {name: 'no-default+edit', flags: [--no-default-features --features edit]}
        {name: 'no-default+tiny-skia,codecs-all', flags: [--no-default-features --features 'tiny-skia,codecs-all']}
        {name: '+javascript', flags: [--features javascript]}
        {name: '+markdown', flags: [--features markdown]}
        {name: '+svg-export', flags: [--features svg-export]}
        {name: '+svg-import', flags: [--features svg-import]}
        {name: '+svg-text', flags: [--features svg-text]}
        {name: '+png', flags: [--features png]}
        {name: '+tiny-skia,agg', flags: [--features 'tiny-skia,agg']}
        {name: '+vello-gpu', flags: [--features vello-gpu]}
        {name: '+full', flags: [--features full]}
        {name: 'all-features', flags: [--all-features]}
    ]
    for c in $combos {
        print $"    ($c.name)"
        ^cargo check -p pdfrum --lib --quiet ...$c.flags
    }
    print "    tests + all-features"
    ^cargo check -p pdfrum --tests --all-features --quiet

    print "==> cargo nextest run (with the tool's and the CLI's javascript features and pdfrum/svg-export)"
    ^cargo nextest run --workspace --locked --features pdfrum-tool/javascript,pdfrum-cli/javascript,pdfrum/svg-export

    # `svg-import` and `svg-text` are off in the run above, and the two test
    # targets that need them (`svg_ingest`, `svg_form`) declare
    # `required-features`, so nextest silently builds neither. Compiling them
    # under `--all-features` is not running them: the whole ingest pipeline
    # and its 21 fixtures were being typechecked and discarded.
    print "==> cargo nextest run (the svg-import targets)"
    ^cargo nextest run -p pdfrum --locked --features 'svg-import,svg-text,tiny-skia'

    print "==> cargo test --doc (nextest silently skips doctests)"
    # `--all-features`, not `pdfrum/svg-export`: a doctest behind a default-off
    # flag is an uncompiled claim, and half the facade's features have some.
    ^cargo test --doc --workspace --locked --all-features

    print "==> cargo doc --no-deps (deny warnings)"
    with-env { RUSTDOCFLAGS: '-D warnings' } {
        ^cargo doc --no-deps --workspace
        # Default-off items (`Error::Svg`, `DocEdit::compile_svg`, …) are
        # absent from the workspace build; this one has to see them.
        ^cargo doc --no-deps -p pdfrum --all-features
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
    # `wayland-sys` is in the GPU tree and carries a build script that links
    # the C libraries — unless `dlopen` is on, which makes it return before it
    # probes anything and leaves the crate a set of `extern` declarations
    # resolved at run time. Exempting it by name alone would keep passing if a
    # `wgpu` bump ever turned that feature off, so the feature is asserted
    # rather than assumed.
    #
    # `renderdoc-sys` carries no build script and no `links` key at all, and
    # `linux-raw-sys` (rustix's constants) and `js-sys` (wasm-bindgen's JS
    # stdlib) are `-sys` by name only. Those three have nothing to assert.
    let dlopen_sys = [wayland-sys]
    let no_build_script_sys = [renderdoc-sys]
    let pure_rust_sys = [linux-raw-sys js-sys]
    let native_build_crates = [cc cmake pkg-config bindgen]

    for c in $dlopen_sys {
        # `cargo tree -e features` prints an enabled feature as its own node,
        # so the feature the exemption rests on is either in the tree or the
        # exemption no longer holds.
        let enabled = (^cargo tree -e features -i $c --workspace
            | lines
            | any {|l| $l =~ $'($c) feature "dlopen"' })
        if not $enabled {
            print --stderr $"error: ($c) is in the tree without its `dlopen` feature,"
            print --stderr "       so its build script links C libraries."
            exit 1
        }
    }

    let gpu_sys_exemptions = ($dlopen_sys | append $no_build_script_sys)

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

    print "==> silent-skip floor"
    ^./scripts/count-silent-skips.nu

    # fuzz/ is its own workspace (libfuzzer-sys / cc). Do not add it to
    # members. This only typechecks; running is scripts/fuzz-gate.nu (weekly
    # workflow, not this gate).
    print "==> cargo check (fuzz workspace)"
    ^cargo check --manifest-path fuzz/Cargo.toml --all-targets

    print ""
    print "CI green."
}
