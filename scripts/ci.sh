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

echo
echo "CI green."
