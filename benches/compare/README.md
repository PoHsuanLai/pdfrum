# pdfrum-compare

Own workspace. Two peers wrap C; do not add this directory to root `members`.

Subject: `cargo add pdfrum` (`pdfrum` row). Extra rows `pdfrum-agg` and
`pdfrum-tinyskia` are the same facade with a different CPU rasterizer;
`pdfrum-vello-gpu` is `--features gpu`. They render only, and SSIM is the
quality column. Tables and method: [`docs/benchmarks/`](../../docs/benchmarks/).

```sh
cargo build --release
cargo build --release --features c-engines
cargo build --release --features gpu
cargo run --release -- run --corpus ../corpus --out /tmp/run.json
cargo run --release -- report /tmp/run.json
cargo run --release -- report /tmp/run.json --losses
cargo run --release -- adoption --out /tmp/adoption.json
cargo run --release -- throughput --corpus ../corpus --out /tmp/throughput.json
```
