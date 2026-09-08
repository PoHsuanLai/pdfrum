# Contributing

## Build

The CI gate's toolchain is `rust-toolchain.toml` (currently 1.98.1). Bump
that file on purpose when clippy or rustc behaviour should change — an
unpinned `stable` makes `#[expect(lint)]` a landmine under `-D warnings`.
The manifest's `rust-version` field (`[workspace.package]` in the root
`Cargo.toml`) is the minimum, checked by a dedicated MSRV job. No system
libraries.

Three commands are enough for a first patch:

```bash
cargo build --workspace
cargo nextest run
cargo test --doc --workspace   # nextest skips doctests
```

`cargo nextest run` needs `cargo binstall cargo-nextest`. Plain `cargo test`
runs the same tests, slower and one process for all of them; nextest gives
each test its own process, which is why a hang or a leak is legible. Neither
tool covers both halves: nextest cannot run doctests at all, so the third
line stands on its own.

`just` wraps these — `cargo install just --locked`, then `just --list`.
`just check` is the three commands above; the recipes are thin, and the ones
that need nushell say so.

## CI

`./scripts/ci.nu` is the full gate. On top of the three commands it runs
fmt, clippy `-D warnings`, the pdfrum feature combination matrix
(`--no-default-features`, each default-off flag, `--all-features`), nextest
with the CLI's and the tool's `javascript` features, rustdoc (workspace
defaults, then pdfrum `--all-features`) and its coverage floor, the API
snapshot, `cargo deny`, the no-`-sys` check, the C header and C test, WASM
tests, and `cargo check` of `fuzz/`.

CI does not run the conformance board or the bench ratchet. The GPU suite
runs on Linux with lavapipe (`PDFRUM_ALLOW_SOFTWARE_GPU=1`); without that
env the tests skip rather than fail. A weekly workflow runs
`scripts/fuzz-gate.nu`.

The gate is long and its tools are several. A maintainer runs it before
landing, so a first patch can skip it. Run it yourself once a change touches
features, the C API, WASM, or dependencies — the parts the three commands do
not see. `cargo fmt --all` and `cargo clippy --workspace --all-targets --
-D warnings` are worth running either way; they are what the gate fails on
first.

The scripts are nushell:

```bash
cargo binstall nu@0.110.0
```

The gate also wants:

```bash
cargo binstall cargo-nextest
cargo install cargo-public-api --locked
rustup toolchain install nightly
```

Optional — the gate notes and continues if they are missing: `cargo-deny`,
`cbindgen`, `wasm-bindgen-cli`, the `wasm32-unknown-unknown` target.

## Conformance

PDFium is a read-only oracle outside this repo. The harness diffs 1759
files on text, structure, pixels (floors in `conformance/thresholds.toml`),
and JS transcripts into `conformance/scoreboard.json`.

Name any board rows a change moves. Silent movement is a regression.
Divergences from PDFium are documented next to the code; where PDFium is
wrong we keep the correct behaviour and mark the row unachievable.

The board needs a multi-hour PDFium build, so a first patch can skip it.
A maintainer runs it before landing.

## Performance

`ratchet check` against `benches/baseline.json` is local. Do not hand-edit
the file; `ratchet update` and say by how much. Comparative numbers:
[`docs/benchmarks/`](docs/benchmarks/).

## Style

`cargo fmt` for formatting. Review also expects:

- Plain data and functions. Enums, exhaustive `match`. No global state.
  PDF cross-refs are ids (`ObjRef`), not `Rc<RefCell>`.
- Three trait seams: `RenderDevice` / `RasterBackend`, `Resolve`, `Cascade`.
  A fourth needs review.
- New code: impossible states do not compile.
- No panics on untrusted input. Damage goes to `Diagnostics`; `Err` means
  stop. An API-contract panic (`Array::insert` with `index > len()`, the
  same shape as `Vec::insert`) is documented at the call site.
- `unsafe_code = "forbid"` except `pdfrum-capi` (see that crate's rustdoc).
- No `-sys`, no C in a library build. Write thirty lines instead of a helper
  crate. GPU is the one exemption: `pdfrum-raster-vello`, never a default dep.
- Port PDFium behaviour, not C++ shape.

Rustdoc is for callers: first sentence, invariant, `# Errors`, one example.
History belongs in `//`.

CLI: stdout is data, stderr is commentary, `--json` is the twin. Print
through `crates/pdfrum-cli/src/out.rs`.

## Public API

`docs/api-baseline/` is the surface `cargo add pdfrum` sees. A drift is its
own commit, from `./scripts/api-snapshot.nu update` (`just api-update`).
That one is not skippable: an API change without a matching baseline fails
the gate. It needs nushell, `cargo-public-api` and nightly — the install
lines are under CI.

Config structs that will grow (`RenderOptions`, `SaveOptions`, `OpenOptions`,
`StampOptions`, `AttachmentOptions`) are `#[non_exhaustive]`. Fill them in
with the builder; `..Default::default()` is a same-crate spelling only.
Closed value types (`Stroke`, `Revision`) stay exhaustive. Enums that may
grow (`LinkTarget`, `ImageEncoding`, `FontFileKind`) are `#[non_exhaustive]`.
Do not mix the two policies on a new type.

## Versions

Internal workspace members depend on each other with a caret on `0.1.0`, so
a patch of one crate is usable with siblings still at 0.1.0. Releases still
bump the workspace together. `kurbo` and `peniko` are carets because their
types are in public signatures; every other external crate is an exact pin.
`Cargo.lock` is what reproduces our own builds. Keep `--locked` in CI.

## Releases

Workspace versions bump together. `scripts/prepare-release.nu` cuts
`CHANGELOG.md` (`[Unreleased]` becomes `[x.y.z] - date`) and, if you pass a
version, writes it into `Cargo.toml`. `just prepare-release` uses the version
already in the manifest; `just prepare-release 0.1.1` bumps first.

Commit that, open a PR, merge to `main`. Once CI is green,
`.github/workflows/tag-release.yml` tags `v*` and the publish workflow
uploads to crates.io and opens the GitHub Release from the changelog
section. Pushing the tag by hand still takes the same publish path.

## Paths

| variable | default |
|---|---|
| `PDFRUM_ORACLE_CHECKOUT` | `<repo>/../pdfium-c++` |
| `PDFRUM_ORACLE_BIN` | `<checkout>/out/Release/pdfium_test` |
| `PDFRUM_GOLDENS` | `<repo>/conformance/goldens` |

Tests that need the oracle skip if the binary is missing.

## Licence

Apache-2.0 OR MIT.
