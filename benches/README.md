# pdfrum-bench

Not published. No library crate depends on it.

| Path | |
|---|---|
| `corpus/` | 44 measurement PDFs. See `corpus/PROVENANCE.md`. |
| `corpus-list/` | Shared document list (`pdfrum-corpus`). |
| `fixtures/` | Seven older documents, kept for table continuity. |
| `src/bin/ratchet.rs` | Compare a run to `baseline.json`. |
| `src/bin/profile.rs` | One operation in a loop (`scripts/profile.nu`). |
| `src/bin/scaling.rs` | One rayon thread count per process. |
| `baseline.json` | Committed medians and noise bands. |

| Crate | Groups |
|---|---|
| `pdfrum-parser` `benches/open.rs` | `open` |
| `pdfrum-page` `benches/build.rs` | `build` |
| `pdfrum-render` `benches/render.rs` | `render-{cold,warm}-{exact,tinyskia,vello}` |
| `pdfrum-text` `benches/text.rs` | `text` |
| `pdfrum-edit` `benches/save.rs` | `save` |

`render-cold` builds a fresh `RenderSession` inside the timed closure.
`render-warm` holds one session across iterations. The oracle column is warm.

```sh
scripts/bench-quick.nu                 # 18 files, ~3 min
cargo bench --workspace                # 44 files × 11 groups, ~1 h
cargo bench -p pdfrum-render
cargo run --release -p pdfrum-bench --bin ratchet -- check
cargo run --release -p pdfrum-bench --bin ratchet -- update
scripts/profile.nu render benches/corpus/text_foxittext.pdf 50 exact
scripts/bench-oracle.nu
```

A filtered `cargo bench` leaves other crates' Criterion results on disk;
`check` will treat them as fresh. Run `--workspace` before a check that
decides anything.

Run-to-run spread reaches 5% on heavy documents. A result inside the band
in `baseline.json` is no result. A parity-engine speed change also needs an
unchanged conformance board.
