## What this changes

## Why

## The gate

- [ ] `./scripts/ci.nu` is green locally.

`scripts/ci.nu` is the definition of done: fmt, clippy with warnings denied,
nextest, doctests, rustdoc, the public-API snapshot, `cargo deny`, the
pure-Rust dependency check, the C ABI test, the WebAssembly tests, and a
`cargo check` of the fuzz workspace. CI runs all of it.

## Conformance

Every change that could move a pixel, a character or a byte is measured against
the board before it lands. The board needs a read-only PDFium checkout, so CI
cannot run it — a maintainer does, locally.

- [ ] Board run, and the row count is unchanged.
- [ ] Board run, and rows moved — the count and the direction are below, with
      the reason.
- [ ] Not applicable: this change cannot move a row (documentation, tooling,
      comments).

Rows moved:

## Performance

- [ ] `ratchet check` is green.
- [ ] Not applicable.

If a baseline moved, say by how much and on which machine. Do not hand-edit
`benches/baseline.json`; use `ratchet update`.

## Public API

- [ ] The API snapshots are unchanged.
- [ ] The snapshots changed, and the change is its own commit made with
      `./scripts/api-snapshot.nu update`.

A snapshot drift is a change to what `cargo add pdfrum` sees. It is never
incidental.
