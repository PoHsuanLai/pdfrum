# Thin wrappers over the commands in CONTRIBUTING.md. `cargo install just --locked`.
# Recipes marked "nushell" shell out to scripts/*.nu; the logic lives there,
# not here, so the two cannot drift.

# List the recipes.
default:
    @just --list

# Build every crate in the workspace.
build:
    cargo build --workspace

# Run the test suite. nextest skips doctests; see `just doctest`.
test:
    cargo nextest run

# Run the doctests, which nextest cannot.
doctest:
    cargo test --doc --workspace

# Build, test, doctest: enough for a first patch.
check: build test doctest

# Format every crate.
fmt:
    cargo fmt --all

# Formatting gate, as CI runs it.
fmt-check:
    cargo fmt --all -- --check

# Lints, warnings denied, as CI runs it.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Public-API baseline against docs/api-baseline/ (nushell, cargo-public-api, nightly).
api-check:
    ./scripts/api-snapshot.nu check

# Regenerate docs/api-baseline/. Required for any public-API change; its own commit.
api-update:
    ./scripts/api-snapshot.nu update

# The whole CI gate (nushell). Long; a maintainer runs it before landing.
ci:
    ./scripts/ci.nu
