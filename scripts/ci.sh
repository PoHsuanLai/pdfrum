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
forbidden=$(cargo tree -e normal --workspace --prefix none \
    | awk '{print $1}' | sort -u \
    | grep -E -- '-sys$|^(cc|cmake|pkg-config|bindgen)$' || true)
if [ -n "$forbidden" ]; then
    echo "error: forbidden native-build crates in the dependency tree:" >&2
    printf '  %s\n' $forbidden >&2
    exit 1
fi
echo "ok: dependency tree is pure Rust"

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
