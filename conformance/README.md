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

Point the harness at the binary `cargo` reports rather than at a path:

```
PDFRUM_TOOL=$(cargo build -p pdfrum-tool --release --features javascript \
  --message-format=json | jq -r 'select(.executable != null) | .executable')
```

`CARGO_TARGET_DIR` moves cargo's output tree, so the `<repo>/target/...`
default below holds either nothing or an artifact from before the redirect.
An absent binary is caught — `run` resolves and probes it before the walk and
refuses to start — but a *stale* one answers the probe and scores the corpus
against code nobody is reading. Every run prints the resolved path and its
mtime, and records both under `tool` in `scoreboard.json`, so a board can be
dated; check that stamp before trusting a pass rate.

| variable | flag | default |
|---|---|---|
| `PDFRUM_ORACLE_CHECKOUT` | `--checkout` | `<repo>/../pdfium-c++` |
| `PDFRUM_ORACLE_BIN` | `--oracle` | `<checkout>/out/Release/pdfium_test` |
| `PDFRUM_GOLDENS` | `--goldens` | `<repo>/conformance/goldens` |
| `PDFRUM_TOOL` | `--tool` | `<repo>/target/release/pdfrum-tool` (see above) |

Flags win over variables. `conformance/goldens/` is gitignored. A run with
no goldens reports `missing-golden` and still exits 0 — write trials to
`--out`. `--out` is a file path.

`run` refuses to write a board that measured nothing — no rows at all, or
effectively every row `unsupported-tool` — and exits non-zero leaving `--out`
untouched, so a regeneration script cannot commit a zeroed scoreboard. It also
warns when the row count drifts far from the reference board's 1759.

The checkout is read-only: commands refuse if any tracked file is dirty
(`--allow-dirty-oracle` to override). `pdfium_test` writes beside its
input, so the harness copies each file to scratch first.

**Divergences.** PDFium is the oracle, not the specification. Where the two
part company and ISO 32000-1 backs pdfrum, matching the oracle would mean
reproducing a known defect, so `conformance/divergences.toml` names the file
and the row is scored `diverged`: it leaves the pass/fail denominator and is
reported as its own count. Every summary prints three numbers — passed,
diverged, failed — and a divergence is never folded into `pass`.

```toml
[divergences]
"resources/whitespace.pdf" = { why = "…", cite = "crbug.com/40643656; …" }
```

Keyed by the corpus-relative path, `#form-events` and `#js-transcript`
suffixes included. The ratchet is one-directional, as in `thresholds.toml`:
both `why` and `cite` are required and neither may be blank, so a row cannot
be added as a bare path to turn a regression green — `cite` names the tracker
issue, the `[oracle-bug]` source location that reasons it, or the write-up
under `docs/upstream/`. Only a row that *failed* is re-labelled; an entry over
a file that passes is inert, and `run` names it so it can be deleted once the
defect is fixed upstream. `triage` skips diverged rows: reproducing an oracle
defect is not a unit of work.

**Tier C** compares `vello_cpu` and `tiny-skia` on our engine, not against
the oracle. Interior pixels must match within 1 count (hard failure). Edge
pixels (neighbourhood proxy, dilated 1 px from the *intersection* of both
images' boundaries) may differ by 8; that share is reported, not gated.

**`mutate-round-trip`** compares two renders of a file we just wrote (the
oracle cannot save). Baseline is a plain save, not the original — saving
already repairs `/Length`. Pixel counts, not SSIM: a large new rect can
move SSIM without a new disagreement.
