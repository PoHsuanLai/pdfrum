# Contributing

## Building

Stable Rust, no external toolchain, no system libraries.

```bash
cargo build --workspace
cargo nextest run                    # the test runner this project uses
cargo test --doc --workspace         # nextest silently SKIPS doctests
```

The scripts are nushell, and a few of the gate's steps want tools that are not
part of a Rust install:

```bash
cargo binstall nu                        # required: the scripts are nushell
cargo install cargo-nextest --locked     # required by the gate
cargo install cargo-public-api --locked  # required by the API snapshot gate
rustup toolchain install nightly         # rustdoc JSON is nightly-only
cargo install cargo-deny --locked        # optional; the gate skips it if absent
cargo install cbindgen --locked          # optional; for the C header check
cargo install wasm-bindgen-cli --locked  # optional; for the WebAssembly tests
rustup target add wasm32-unknown-unknown # optional; for the WebAssembly tests
```

Each optional tool the gate cannot find becomes a printed note and the run
continues. A contributor who never touches the C ABI should not need `cbindgen`
to send a patch.

## The gate

```bash
./scripts/ci.nu
```

That is the definition of done for every change. It runs, in order: `cargo fmt
--check`; `cargo clippy --workspace --all-targets` with warnings denied;
`cargo nextest run` with the tool's and the CLI's `javascript` features, which
are `cfg`'d off otherwise and would be silently skipped; `cargo test --doc`,
because nextest cannot run doctests and does not say it skipped them; `cargo
doc --no-deps` with warnings denied, which is the only gate that catches a
broken intra-doc link; the public-API snapshot check; `cargo deny check`; the
pure-Rust dependency-tree check; the C header and C test; the WebAssembly
tests; the checks that `wgpu` and `boa` have not leaked into a default build;
and a `cargo check` of the `fuzz/` workspace, which is a separate workspace
nothing else reaches.

CI runs the same script. Two things it cannot run are described below.

## The conformance harness

pdfrum is a rewrite of PDFium, and PDFium is kept alongside as a differential
test oracle: a read-only source checkout and a built `pdfium_test` binary, both
outside this repository. The harness runs both engines over about 1750 files
and compares them on four tiers — extracted text byte for byte, structural
dumps byte for byte, rendered pixels by SSIM against a 0.99 floor, and
JavaScript transcripts. The result is recorded in
`conformance/scoreboard.json`.

**Every change is measured against it.** That is the point of the project. The
folklore this engine exists to preserve — what to do when the cross-reference
table lies, which of a damaged page tree's contradictions to believe — is not
written down anywhere except in PDFium's behaviour, so a change that "looks
right" and moves twelve board rows is a regression whether or not any test in
`cargo nextest` noticed. A change that moves rows is not forbidden; a change
that moves rows silently is.

Where we deliberately differ from PDFium, the divergence is stated in the
crate's own documentation next to the code, with the reason. Where PDFium is
simply wrong, we implement the correct behaviour, cite both, and record the
board row as not achievable rather than pretending it passes.

Building the oracle is a multi-hour Chromium-style build, so CI does not run
the board and neither does a first-time contributor. Open a pull request
without it and say so; a maintainer runs it before landing.

## Performance

`benches/baseline.json` holds medians taken on one reference machine, and
`ratchet check` compares against them. A shared CI runner's numbers mean
nothing against those, so this is local too. If a baseline genuinely moved, use
`ratchet update` — never hand-edit the file — and say in the pull request by
how much and on which machine.

`docs/benchmarks/` publishes the comparative numbers against other engines, and
`docs/benchmarks/losses-explained.md` explains every row where pdfrum loses.
Losses are not omitted from those tables. If your change makes something
slower, the honest thing is to measure it and write it down.

## Style

`STYLE.md` is binding, and a violation blocks review even when the tests pass.
It is not a formatting document — `cargo fmt` handles that — but a set of
design rules: plain data types transformed by functions, enums over class
hierarchies, no global state, a closed list of three trait seams, and new code
written so an impossible state does not compile.

`SPEC.md` holds the concrete per-crate type contracts and the protocol for
changing one. `DEPS.md` is the closed dependency set: adding a crate is a
decision, with the rationale and the rejected alternatives written down.

`docs/design/cli-style.md` governs every byte a command prints. There is one
output style, four layout forms and a fixed palette, and printing happens
through the helpers in `crates/pdfrum-cli/src/out.rs` and nothing else.

## The public API

`cargo add pdfrum` sees a snapshot committed to this repository. A drift in it
is a change to the library's public surface and is never incidental: make it
its own commit, produced by `./scripts/api-snapshot.nu update`, so a reviewer
sees exactly what moved.

## Sending a change

Branch, commit, open a pull request against `main`. The pull request template
asks for the gate, the board and the ratchet; fill in what applies and say
plainly what does not.

Small, well-measured changes land quickly. A change with a number attached
lands faster than one with an argument attached.

## Open work

`docs/issues-to-file.md` is the open work list: every known gap written as an
issue, with the measurement that shows it and what would count as done. Several
entries say "not determined" and name the experiment that would determine them.
Those are good places to start.

## Licence

Contributions are dual-licensed under Apache-2.0 and MIT, matching the project.
