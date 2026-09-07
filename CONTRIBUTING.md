# Contributing

## Build

Stable Rust. No system libraries.

```bash
cargo build --workspace
cargo nextest run
cargo test --doc --workspace   # nextest skips doctests
./scripts/ci.nu                # what CI runs
```

Scripts are nushell (`cargo binstall nu`, ≥ 0.110). The gate also wants:

```bash
cargo binstall cargo-nextest
cargo install cargo-public-api --locked
rustup toolchain install nightly
```

Optional — the gate notes and continues if they are missing: `cargo-deny`,
`cbindgen`, `wasm-bindgen-cli`, the `wasm32-unknown-unknown` target.

## CI

`./scripts/ci.nu` runs fmt, clippy `-D warnings`, nextest (with `javascript`
on the CLI and the tool), doctests, rustdoc, the API snapshot, `cargo deny`,
the no-`-sys` check, the C header and C test, WASM tests, and
`cargo check` of `fuzz/`.

CI does not run the conformance board or the bench ratchet.

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
- No panics in library crates. Damage goes to `Diagnostics`; `Err` means stop.
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
own commit, from `./scripts/api-snapshot.nu update`.

## Paths

| variable | default |
|---|---|
| `PDFRUM_ORACLE_CHECKOUT` | `<repo>/../pdfium-c++` |
| `PDFRUM_ORACLE_BIN` | `<checkout>/out/Release/pdfium_test` |
| `PDFRUM_GOLDENS` | `<repo>/conformance/goldens` |

Tests that need the oracle skip if the binary is missing.

## Licence

Apache-2.0 OR MIT.
