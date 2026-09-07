## What this changes

## Why

## The gate

- [ ] `./scripts/ci.nu` is green locally.

## Conformance

CI does not run the board. A maintainer does before landing.

- [ ] Board run, row count unchanged.
- [ ] Board run, rows moved (count, direction, reason below).
- [ ] N/A (docs, tooling, comments).

Rows moved:

## Performance

- [ ] `ratchet check` is green.
- [ ] N/A.

If a baseline moved, say by how much and on which machine. Use
`ratchet update`, do not hand-edit `benches/baseline.json`.

## Public API

- [ ] `docs/api-baseline/` unchanged.
- [ ] Snapshots changed in their own commit (`./scripts/api-snapshot.nu update`).
