# Benchmarks

Three jobs.

| Job | Where |
|---|---|
| Is this commit slower? | Criterion groups on the crate they measure. `ratchet` vs `baseline.json`. |
| Where does the time go? | `profile` binary (`scripts/profile.nu`). |
| How do we compare to peers? | `compare/` — own workspace. Engines, tables, numbers: [`docs/benchmarks/`](../docs/benchmarks/). |

The 44 PDFs are `corpus/`. One list (`pdfrum-corpus`) so every number is the same files. `fixtures/` is a smaller set, not the ratchet.

Criterion groups live with the code:

| Crate | Groups |
|---|---|
| `pdfrum-parser` | `open` |
| `pdfrum-page` | `build` |
| `pdfrum-render` | `render-{cold,warm}-{agg,tinyskia,vello-cpu}` |
| `pdfrum-text` | `text` |
| `pdfrum-edit` | `save` |

`render-cold` builds a fresh `RenderSession` inside the timed closure.
`render-warm` holds one session across iterations. The oracle column is warm.

```sh
cargo bench --workspace                # 44 files × 11 groups, ~1 h
cargo bench -p pdfrum-render
cargo run --release -p pdfrum-bench --bin ratchet -- check
cargo run --release -p pdfrum-bench --bin ratchet -- update
scripts/profile.nu render benches/corpus/text_foxittext.pdf 50 agg
cargo run --release -p pdfrum-bench --bin scaling -- --threads 8
```

`cargo bench -p` one crate leaves other groups' Criterion files on disk;
`check` treats them as fresh. Run `--workspace` before a check that decides
anything.

Run-to-run spread reaches 5% on heavy documents. A result inside the band
in `baseline.json` is no result.

`compare/` is not a workspace member: two peers wrap C. Do not add it to
root `members`. Commands: [`compare/README.md`](compare/README.md).
