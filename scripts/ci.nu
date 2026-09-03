#!/usr/bin/env nu
# CI gate for pdfrum (PLAN.md §7 definition of done).
# Runs: fmt, clippy -D warnings, nextest, doctests, cargo-deny (if installed),
# and the pure-Rust dependency-tree check from DEPS.md.
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

    print "==> cargo nextest run"
    ^cargo nextest run

    print "==> cargo test --doc (nextest silently skips doctests)"
    ^cargo test --doc --workspace

    # Doctests prove the *examples* compile and run; this proves the prose
    # around them resolves. A broken intra-doc link is invisible to every other
    # gate here — it degrades to plain text in the rendered page — so without
    # this the docs rot silently while the build stays green.
    print "==> cargo doc --no-deps (deny warnings)"
    with-env { RUSTDOCFLAGS: '-D warnings' } {
        ^cargo doc --no-deps --workspace
    }

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
    let native_build_crates = [cc cmake pkg-config bindgen]

    let forbidden = (^cargo tree -e normal --workspace --prefix none
        | lines | split column ' ' name | get name | uniq
        | where {|c| ($c | str ends-with '-sys') or ($c in $native_build_crates) }
        | where {|c| $c not-in $gpu_sys_exemptions }
        | sort)
    if not ($forbidden | is-empty) {
        print --stderr "error: forbidden native-build crates in the dependency tree:"
        $forbidden | each {|c| print --stderr $"  ($c)" } | ignore
        exit 1
    }
    print "ok: dependency tree is pure Rust"

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
    # pulls `cc` — both of which the filter above would reject. DEPS.md
    # sanctions that only outside the library ring, and `--workspace` here never
    # reaches fuzz/ because it is not a member. Do not add it to the root
    # Cargo.toml's `members`.
    #
    # *Running* the fuzz ring is not part of this gate: it is minutes to days
    # of work where this script is seconds. Its own gate is
    # scripts/fuzz-gate.nu, and PLAN.md §6 makes
    # `scripts/fuzz-gate.nu 86400 parallel` an M1 exit criterion. See
    # fuzz/README.md for how to run and reproduce.
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
