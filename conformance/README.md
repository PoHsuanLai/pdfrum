# Conformance harness

```
conformance generate-goldens
conformance run
conformance run --check-regressions conformance/scoreboard.json
conformance triage
conformance tier-c
conformance save-round-trip
conformance mutate-round-trip
```

Build the tool with its own feature: `cargo build -p pdfrum-tool --release
--features javascript`. `--features pdfrum/javascript` compiles and silently
drops JS-transcript rows.

| variable | flag | default |
|---|---|---|
| `PDFRUM_ORACLE_CHECKOUT` | `--checkout` | `<repo>/../pdfium-c++` |
| `PDFRUM_ORACLE_BIN` | `--oracle` | `<checkout>/out/Release/pdfium_test` |
| `PDFRUM_GOLDENS` | `--goldens` | `<repo>/conformance/goldens` |
| `PDFRUM_TOOL` | `--tool` | `<repo>/target/release/pdfrum-tool` |

Flags win over variables. `conformance/goldens/` is gitignored. A run with
no goldens reports `missing-golden` and still exits 0 — write trials to
`--out`. `--out` is a file path.

The checkout is read-only: commands refuse if any tracked file is dirty
(`--allow-dirty-oracle` to override). `pdfium_test` writes beside its
input, so the harness copies each file to scratch first.

**Tier C** compares `vello_cpu` and `tiny-skia` on our engine, not against
the oracle. Interior pixels must match within 1 count (hard failure). Edge
pixels (neighbourhood proxy, dilated 1 px from the *intersection* of both
images' boundaries) may differ by 8; that share is reported, not gated.

**`mutate-round-trip`** compares two renders of a file we just wrote (the
oracle cannot save). Baseline is a plain save, not the original — saving
already repairs `/Length`. Pixel counts, not SSIM: a large new rect can
move SSIM without a new disagreement.
