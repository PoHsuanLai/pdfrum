# pdfrum-script

Acrobat's `AF*` field-format, keystroke, validate and calculate helpers, and the
`util.printf` / `util.printd` / `util.printx` / `util.scand` formatters, as
**pure Rust functions**.

These are the routines a PDF's own form scripts call. Here they are plain
functions over plain data: no JavaScript engine, no form session, no field
lookup, no clock, and no locale. A date function is handed the current time as a
parameter rather than reading one, which is what makes the crate deterministic
and testable without a document.

The two things Acrobat does through its host that a pure function cannot — raise
an alert, and recolour a field's text — come back as **data**, in `AfEffects`.
Nothing is performed as a side effect and nothing is discarded, so a caller with
a real field can reproduce the whole behaviour and a caller without one can
ignore the parts it cannot honour. That matters most for the negative number
style that prints no minus sign at all and carries the sign entirely in the
colour.

The error message strings are compared verbatim by the conformance transcripts,
so they are API rather than diagnostics; `error.rs` says so and must not be
tidied.

Six of the oracle's behaviours are defects rather than Adobe's specification —
an inverted leap-year rule, a hardcoded two-digit-year window, noon labelled
`am`, an operation name whose case changes the answer, a weekday always printed
as Sunday, and the empty string reported as a number. This crate implements the
correct behaviour, marks each divergence `// [oracle-bug]` at its site with both
citations, and records the cost against the goldens in the internal working notes.
