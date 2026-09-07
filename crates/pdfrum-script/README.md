# pdfrum-script

Acrobat's form-script built-ins as pure functions: the `AF*` family
(ISO 32000-1 §12.7.5.3 field formatting, keystroke, validate and calculate
actions) and `util.printf` / `printd` / `printx` / `scand`. No engine, no
session, no clock, no document — arguments in, a value out.

```rust
use pdfrum_script::{AfOutcome, af_number_format};

let out = af_number_format("1234.5", 2, 0, 0, "$", true);
assert_eq!(out.outcome, AfOutcome::Formatted("$1,234.50".into()));
assert!(out.effects.is_empty());
```

Every function is pure because these are the parts of a form script that must
behave identically whether or not a JavaScript engine is present. A format
action's effects on the world — an alert, a recoloured field — come back as
data in [`AfEffects`] rather than being performed, so the caller decides
whether a headless extraction shows a dialog. Two functions can therefore be
tested against the oracle's transcripts with no engine at all, and the error
*strings* are part of the API for exactly that reason: a golden transcript
compares them literally.

Acrobat gets some of these wrong — leap years, two-digit year windows,
weekday arithmetic. Those defects are implemented **correctly** here and the
divergence is marked `// [oracle-bug]` at the site, so a conformance row that
fails is a documented disagreement rather than an unexplained one.

The facade's `javascript` feature is what pulls this crate — and
[boa](https://crates.io/crates/boa_engine) — into a `cargo add pdfrum` tree.
With the feature off, a document's scripts are data.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
