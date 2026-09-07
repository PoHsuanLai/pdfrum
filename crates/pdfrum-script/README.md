# pdfrum-script

Acrobat `AF*` helpers and `util.printf` / `printd` / `printx` / `scand` as
pure functions. No engine, no session, no clock. Alerts and recolouring
come back in `AfEffects`. Error strings are API (transcript goldens).

Oracle defects (leap year, two-digit years, weekday, …) are implemented
correctly and marked `// [oracle-bug]` at the site.

```rust
use pdfrum_script::{AfOutcome, af_number_format};

let out = af_number_format("1234.5", 2, 0, 0, "$", true);
assert_eq!(out.outcome, AfOutcome::Formatted("$1,234.50".into()));
assert!(out.effects.is_empty());
```

The facade's `javascript` feature is what pulls this crate (and boa) into
a `cargo add pdfrum` tree. `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
