#!/usr/bin/env nu
# CI gate for pdfrum.
# Runs: fmt, clippy -D warnings, nextest, doctests, cargo-deny (if installed),
# and the pure-Rust dependency-tree check.
#
# Fail-fast: nushell aborts a script when an external command exits non-zero,
# which is this file's `set -euo pipefail`. Every step below is therefore a
# bare external call and the first red one stops the run with its own exit
# code — except the two places that deliberately do not fail, both marked.

def main [] {
    cd ($env.FILE_PWD | path dirname)

    print "==> cargo fmt --check"
    ^cargo fmt --all -- --check

    print "==> cargo clippy (deny warnings)"
    ^cargo clippy --workspace --all-targets -- -D warnings

    # The tool's and the CLI's JavaScript tests are `cfg`'d behind their
    # `javascript` features, and the facade's SVG test behind `pdfrum/svg`;
    # without the flags they never run and the gate would pass over them.
    print "==> cargo nextest run (with the tool's and the CLI's javascript features and pdfrum/svg)"
    ^cargo nextest run --workspace --features pdfrum-tool/javascript,pdfrum-cli/javascript,pdfrum/svg

    print "==> cargo test --doc (nextest silently skips doctests)"
    ^cargo test --doc --workspace --features pdfrum/svg

    # Doctests prove the *examples* compile and run; this proves the prose
    # around them resolves. A broken intra-doc link is invisible to every other
    # gate here — it degrades to plain text in the rendered page — so without
    # this the docs rot silently while the build stays green.
    print "==> cargo doc --no-deps (deny warnings)"
    with-env { RUSTDOCFLAGS: '-D warnings' } {
        ^cargo doc --no-deps --workspace
    }

    # `cargo doc` above proves the docs *build*; this proves they are *there*.
    # Neither `missing_docs` nor `-D warnings` sees an undocumented struct
    # field or a public method with no example, and both regress one item at a
    # time. Nightly-only tooling, so it stands down with a note on stable —
    # the script says why.
    ^./scripts/check-rustdoc-coverage.nu

    # WP13. The committed snapshots are the public API; a drift is a change
    # to what `cargo add pdfrum` sees. The script's header used to forbid
    # this line — that sentence became false when the surface settled.
    ^./scripts/api-snapshot.nu check

    print "==> cargo deny check"
    # The one step that warns and continues rather than failing: the audit is
    # optional tooling, and a contributor without it still gets the rest of the
    # gate. Do not turn this into a hard failure without saying so in README.md.
    if (which cargo-deny | is-empty) {
        print --stderr "warning: cargo-deny not installed; skipping license/ban audit"
        print --stderr "         install with: cargo install cargo-deny --locked"
    } else {
        ^cargo deny check
    }

    print "==> pure-Rust check (no -sys / cc / cmake / pkg-config / bindgen)"
    # M12c scopes one exemption to `pdfrum-raster-vello` and bounds it two
    # ways. The `cc`/`cmake`/`pkg-config`/`bindgen` half of the guarantee is NOT
    # relaxed — measured 2026-08-31, the whole vello/wgpu tree has none of them
    # as a build dependency, so no C is compiled and nothing below changes for
    # them.
    #
    # What the GPU tree does bring is two `-sys` crates, `renderdoc-sys` and
    # `wayland-sys`, and both are `dlopen`-style declaration shims rather than
    # bindings to a compiled library. They are exempted **by name**, not by
    # pattern: a third one arriving is a decision someone should have to make
    # deliberately, and adding it here is how they make it. Every other `*-sys`
    # crate in the workspace, in this crate's tree or anyone else's, still fails.
    #
    # The other half of the bound — that the core ring never reaches this tree
    # at all — is scripts/check-no-wgpu.nu, run below.
    let gpu_sys_exemptions = [renderdoc-sys wayland-sys]
    # `linux-raw-sys` is the name's other false positive: rustix's generated
    # Linux syscall constants, `build = false`, Rust sources only, reached
    # through `crossterm` for `pdfrum view`. Checked 2026-09-05.
    #
    # `js-sys` is the third, and the `-sys` in it means something else again:
    # it is wasm-bindgen's binding to the JavaScript *standard library*, not to
    # a C one. There is nothing native to bind — the "foreign" side is the host
    # engine, reached through wasm-bindgen's imports. It has no build script at
    # all (checked 2026-09-05: no `build.rs` in the published crate), so there
    # is nothing for it to compile even in principle. Reached through
    # `crates/pdfrum-wasm` (M22 phase 4).
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

    # M22 phase 2: the C library, proved by a C program rather than by a Rust
    # test asserting things about one. It compiles `crates/pdfrum-capi/ctest`
    # against the generated header, links the built `libpdfrum.so`, and checks
    # what only C can check — that the header's declarations match the
    # library's symbols, that a `#[repr(C)]` struct reads the same from both
    # sides, and that eight pthreads each rendering their own page of one
    # shared document produce pixmaps identical to a single-threaded render.
    #
    # A C compiler is a **test-time** tool, not a build dependency. The
    # pure-Rust guarantee is about what the library ships, and nothing in
    # `pdfrum-capi`'s own build touches `cc` — the check above still walks this
    # crate's tree and still passes. So a contributor without a C compiler gets
    # a printed note and the rest of the gate, the same bargain `cargo deny`
    # gets above.
    # The C header is generated and committed, and this is the pair of checks
    # that keeps both halves honest: the committed header still matches what
    # cbindgen makes of the source, and the header's declarations still match
    # the library's exported symbols exactly. It needs the release cdylib the
    # C test builds below, so it runs first only in the sense of reading it —
    # `run.sh` builds it, and this asks for it after.
    #
    # Skipped, like `cargo deny`, when the tool is absent: a contributor who
    # never touches the C ABI should not need `cbindgen` to run the gate.
    print "==> the C header (generated, committed, and matching the library)"
    if (which cbindgen | is-empty) {
        print --stderr "warning: cbindgen not installed; skipping the C header check"
        print --stderr "         install with: cargo install cbindgen --locked"
    } else if (which cc | is-empty) {
        print --stderr "warning: no C compiler, so no libpdfrum.so was built;"
        print --stderr "         skipping the C header check"
    } else {
        # `run.sh` builds the release cdylib the symbol half reads.
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

    # M22 phase 4: the WebAssembly binding, proved where it runs. A
    # `#[wasm_bindgen]` export does not exist until a JavaScript runtime
    # instantiates the module, so no host-target test can see one — the same
    # argument the C test makes, in the other direction. These run on
    # `wasm32-unknown-unknown` under Node, through `wasm-bindgen-test-runner`.
    #
    # `cd` into the crate is load-bearing: the runner is named in
    # `crates/pdfrum-wasm/.cargo/config.toml`, and cargo reads a `.cargo/config.toml`
    # relative to the *invocation* directory, not the manifest. Run from the
    # root with `--manifest-path` the tests build and then fail to execute with
    # "Exec format error", because cargo tries to run the `.wasm` itself.
    #
    # Node and the wasm target are tools, not dependencies, so a contributor without either gets a printed note
    # and the rest of the gate — the bargain `cargo deny` and the C test get
    # above.
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

    # The exemption above is only tolerable because it cannot reach an embedder
    # who did not ask for it. That is the claim, and this is the check.
    ^./scripts/check-no-wgpu.nu

    # M15's engine is isolated by a cargo feature rather than by a crate nothing
    # depends on, which is the weaker of the two mechanisms — feature
    # unification means any workspace member can turn it on for the whole
    # build. This is the check that says whether one has.
    ^./scripts/check-no-boa.nu

    # Note the check above passes *because* fuzz/ is its own workspace. It
    # brings in `libfuzzer-sys`, which links LLVM's C++ libFuzzer runtime and
    # pulls `cc` — both of which the filter above would reject. That is
    # sanctioned only outside the library ring, and `--workspace` here never
    # reaches fuzz/ because it is not a member. Do not add it to the root
    # Cargo.toml's `members`.
    #
    # *Running* the fuzz ring is not part of this gate: it is minutes to days
    # of work where this script is seconds. Its own gate is
    # scripts/fuzz-gate.nu. See fuzz/README.md for how to run and reproduce.
    #
    # *Compiling* it is seconds, so it belongs here. The targets call library
    # API directly — deeper than any caller-facing surface — and being outside
    # every gate is how they silently rotted through an API change: nothing
    # built them until someone reached for the fuzzer. A stable `cargo check`
    # is the whole gate, because a target that does not typecheck is a target
    # `cargo +nightly fuzz build` cannot build either. `--all-targets` so the
    # `fuzz_target!` bodies get checked in their `test` cfg too.
    print "==> cargo check (fuzz workspace)"
    ^cargo check --manifest-path fuzz/Cargo.toml --all-targets

    print ""
    print "CI green."
}
