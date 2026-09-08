# Public-API baseline

The surface `cargo add` sees, as `cargo public-api` prints it. CI runs
`./scripts/api-snapshot.nu check`. A drift is a red run.

One file per published library crate. Featured surfaces (the same crate with
a feature that adds public items a caller is told to turn on) get a second
file: `pdfrum+javascript.txt`, `pdfrum+png.txt`, `pdfrum+tiny-skia+agg.txt`,
and the codec / `system-fonts` / `png` files for the member crates.

Not here: `publish = false` crates — the C ABI (`pdfrum-capi`), the
WebAssembly binding (`pdfrum-wasm`) and the internal tool (`pdfrum-tool`) —
`#[doc(hidden)]` items, and auto-trait impls (`-sss`).
`Send + Sync` is asserted by a unit test on the facade, not by these files.

```
cargo install cargo-public-api --locked   # nightly; not a workspace dep
./scripts/api-snapshot.nu check           # CI
./scripts/api-snapshot.nu update          # rewrite from the working tree
./scripts/api-snapshot.nu list
```

`update` is a change to what `cargo add pdfrum` sees. Commit it on purpose.
