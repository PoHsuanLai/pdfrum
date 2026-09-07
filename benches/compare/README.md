# pdfrum-compare

Own workspace. Two peers wrap C; do not add this directory to root `members`.

Subject: `cargo add pdfrum`. Tables and method:
[`docs/benchmarks/`](../../docs/benchmarks/).

```sh
cargo build --release
cargo build --release --features c-engines
cargo run --release -- run --corpus ../corpus --out /tmp/run.json
cargo run --release -- report /tmp/run.json
cargo run --release -- report /tmp/run.json --losses
cargo run --release -- adoption --out /tmp/adoption.json
cargo run --release -- throughput --corpus ../corpus --out /tmp/throughput.json
```
