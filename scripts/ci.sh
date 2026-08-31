#!/usr/bin/env bash
# CI gate for pdfrum (PLAN.md §7 definition of done).
# Runs: fmt, clippy -D warnings, nextest, doctests, cargo-deny (if installed),
# and the pure-Rust dependency-tree check from DEPS.md.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy (deny warnings)"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo nextest run"
cargo nextest run

echo "==> cargo test --doc (nextest silently skips doctests)"
cargo test --doc --workspace

# Doctests prove the *examples* compile and run; this proves the prose around
# them resolves. A broken intra-doc link is invisible to every other gate here
# — it degrades to plain text in the rendered page — so without this the docs
# rot silently while the build stays green.
echo "==> cargo doc --no-deps (deny warnings)"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace

echo "==> cargo deny check"
if command -v cargo-deny >/dev/null 2>&1; then
    cargo deny check
else
    echo "warning: cargo-deny not installed; skipping license/ban audit" >&2
    echo "         install with: cargo install cargo-deny --locked" >&2
fi

echo "==> pure-Rust check (no -sys / cc / cmake / pkg-config / bindgen)"
# M12c scopes one exemption to `pdfrum-raster-vello-gpu` and bounds it two
# ways. The `cc`/`cmake`/`pkg-config`/`bindgen` half of the guarantee is NOT
# relaxed — measured 2026-08-31, the whole vello/wgpu tree has none of them as
# a build dependency, so no C is compiled and nothing below changes for them.
#
# What the GPU tree does bring is two `-sys` crates, `renderdoc-sys` and
# `wayland-sys`, and both are `dlopen`-style declaration shims rather than
# bindings to a compiled library. They are exempted **by name**, not by
# pattern: a third one arriving is a decision someone should have to make
# deliberately, and adding it here is how they make it. Every other `*-sys`
# crate in the workspace, in this crate's tree or anyone else's, still fails.
#
# The other half of the bound — that the core ring never reaches this tree at
# all — is scripts/check-no-wgpu.sh, run below.
gpu_sys_exemptions='^(renderdoc-sys|wayland-sys)$'
forbidden=$(cargo tree -e normal --workspace --prefix none \
    | awk '{print $1}' | sort -u \
    | grep -E -- '-sys$|^(cc|cmake|pkg-config|bindgen)$' \
    | grep -E -v -- "$gpu_sys_exemptions" || true)
if [ -n "$forbidden" ]; then
    echo "error: forbidden native-build crates in the dependency tree:" >&2
    printf '  %s\n' $forbidden >&2
    exit 1
fi
echo "ok: dependency tree is pure Rust"

# The exemption above is only tolerable because it cannot reach an embedder who
# did not ask for it. That is the claim, and this is the check.
./scripts/check-no-wgpu.sh

# Note the check above passes *because* fuzz/ is its own workspace. It brings
# in `libfuzzer-sys`, which links LLVM's C++ libFuzzer runtime and pulls `cc`
# — both of which this grep would reject. DEPS.md sanctions that only outside
# the library ring, and `--workspace` here never reaches fuzz/ because it is
# not a member. Do not add it to the root Cargo.toml's `members`.
#
# The fuzz ring is not part of this gate: it is minutes to days of work where
# this script is seconds. Its own gate is scripts/fuzz-gate.sh, and PLAN.md
# §6 makes `scripts/fuzz-gate.sh 86400 parallel` an M1 exit criterion. See
# fuzz/README.md for how to run and reproduce.

echo
echo "CI green."
