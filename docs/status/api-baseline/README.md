# The public-API baseline

**Status:** a measurement, taken 2026-09-02 at commit `9b8f74b`.
**These files are a "before" picture, not an aspiration.**

One file per published library crate, each holding that crate's complete
public API as `cargo public-api` prints it. `pdfrum.txt` is the facade — the
surface a `cargo add pdfrum` caller sees — and the rest are the member crates
it composes.

---

## Why this exists, and why it is not a gate

[`docs/design/idiomatic-api.md`](../../design/idiomatic-api.md) plans thirteen
work packages that break the public surface **on purpose**: newtypes for
indices and versions, an `Event` enum instead of Win32-shaped mouse methods,
one geometry vocabulary, six render methods collapsed into two. Its §7
sequence puts `cargo public-api` snapshotting last, as WP13, and calls it a
drift gate.

That ordering is right for a *gate* and wrong for a *baseline*. WP13
snapshots "the surface we meant to keep" — the after picture. If nobody
records the before, then each package's diff is measured against a surface
that has already moved under it, and no one can say what a given package
actually changed. So the baseline is taken first, and these files are it.

Most of what they record is what the thirteen packages exist to remove. A
line here is not an endorsement. `RenderOptions::no_path_smooth`,
`FormSession::on_button(…, down: bool, …)`, `Document::version() -> u8`, and
the seven `pdfrum::Error` variants wrapping unnameable foreign error types are
all in these files, and all of them are targets.

`scripts/api-snapshot.nu` regenerates and diffs them. It is **deliberately not
wired into `scripts/ci.nu`** — a drift check today would turn every intentional
work-package break into a red CI run. WP13 is where the same command becomes a
gate.

---

## How they were produced

```
cargo +nightly public-api -sss -p <crate>
```

| | |
|---|---|
| `cargo-public-api` | 0.52.0 |
| toolchain | `nightly-x86_64-unknown-linux-gnu` |
| rustc | 1.99.0-nightly (`ba28ff76f` 2026-08-13) |
| cargo | 1.99.0-nightly (`eb98b54bc` 2026-08-11) |
| workspace commit | `9b8f74b` |
| features | default (see below) |

`cargo-public-api` is developer tooling and lives in `~/.cargo/bin`. It is
**not** in `DEPS.md`, is not a workspace dependency, and must never appear in
any crate's `Cargo.toml`. Install it with:

```
cargo install cargo-public-api --locked
```

Nightly is required because the tool reads rustdoc's JSON output, which is a
nightly-only feature. Nothing else in this workspace needs nightly.

### What `-sss` omits, and why

`-sss` is `--omit blanket-impls,auto-trait-impls,auto-derived-impls`. Without
it roughly half of every file is `impl<T, U> Into<U> for T`, `impl Send for
…`, and derived `Clone`/`Debug`/`PartialEq` — identical for every type, and
uninformative about the surface. `pdfrum-common` is 392 lines raw and 167
lines simplified; the 167 are the API a reader would write down by hand, which
is what STYLE.md §4's "one screen a reviewer can read" is measured against.

The trade is that an auto-trait regression — a public type quietly losing
`Send` — does not appear in the diff. The facade already carries a unit test
asserting `Send + Sync` across its public types, and that is both the check
for that property and a better one than a four-hundred-line diff nobody reads.

### What else is not here

- **`#[doc(hidden)]` items.** `cargo public-api` omits them, so
  `pdfrum_text::debug_runs` — which is `#[doc(hidden)]` — is absent. These
  files measure the *documented* surface. That is the right measure for
  WP11, whose rule 3 prescribes `#[doc(hidden)]` as the remedy for oracle
  dump formats: applying the remedy is supposed to shrink the number.
- **Non-default features.** Two exist workspace-wide and neither is on:
  `pdfrum-form/script` (the JavaScript engine, kept off by
  `scripts/check-no-boa.nu`) and `pdfrum-render/walk-profile`. The default
  surface is what `cargo add` gets, and so is what is recorded.

---

## Which crates are here, and which are not

Eighteen files, one per workspace member that publishes a library. Three
members are absent, for two different reasons:

| Crate | Why absent |
|---|---|
| `pdfrum-raster-vello` | `publish = false`. Nothing can `cargo add` it. |
| `pdfrum-script` | `publish = false`. Same. |
| `pdfrum-tool` | No library target — a binary has no public API, and `cargo public-api` errors rather than emitting an empty file. |

`docs/design/idiomatic-api.md` §4 rules that the published member crates stay
published and only their *surface* shrinks. The two `publish = false` crates
were never in that set; §5 WP11 names only `pdfrum-raster-vello`, so
`pdfrum-script` being the second one is worth knowing. `conformance/` and
`benches/` are workspace members too and are excluded for both reasons at once.

The script derives this list from the manifests rather than hardcoding it, so
a crate added to the workspace appears in the baseline without anyone
remembering to edit a list.

---

## Regenerating and diffing

```
./scripts/api-snapshot.nu            # diff the working tree against these files
./scripts/api-snapshot.nu update     # rewrite these files from the working tree
./scripts/api-snapshot.nu list       # the per-crate item counts, as a table
```

`diff` exits non-zero when the surface has moved, and prints the added and
removed items per crate. That output is the review: run it after a work
package lands and it is that package's blast radius, item by item.

`update` rewrites every file. Read its `git diff` before committing — a change
here is a change to what `cargo add pdfrum` sees — and name the work package
in the commit message, so the history says which of the thirteen a given
surface change was.

---

## What the numbers said when they were taken

The counts below are why WP11 exists, and they quantify a claim STYLE.md §4
makes without a number: "public API of every crate fits in one `lib.rs`
re-export block a reviewer can read in one screen."

| Crate | Items | `pub mod` | `pub use` |
|---|---:|---:|---:|
| `pdfrum` | 347 | 2 | 42 |
| `pdfrum-page` | 1470 | 19 | 0 |
| `pdfrum-doc` | 1267 | 49 | 0 |
| `pdfrum-form` | 1255 | 24 | 3 |
| `pdfrum-render` | 707 | 31 | 0 |
| `pdfrum-font` | 549 | 5 | 2 |
| `pdfrum-object` | 410 | 3 | 0 |
| `pdfrum-edit` | 374 | 16 | 0 |
| `pdfrum-common` | 167 | 1 | 1 |
| `pdfrum-text` | 167 | 5 | 0 |
| `pdfrum-parser` | 142 | 1 | 0 |
| `pdfrum-crypt` | 93 | 1 | 0 |
| `pdfrum-filters` | 81 | 1 | 0 |
| `pdfrum-type1` | 74 | 1 | 0 |
| `pdfrum-cmap` | 68 | 2 | 0 |
| `pdfrum-raster-agg` | 43 | 3 | 0 |
| `pdfrum-raster-vello-cpu` | 24 | 1 | 0 |
| `pdfrum-raster-tinyskia` | 23 | 1 | 0 |

`pub use` is the shape STYLE.md §4 asks for and `pub mod` is its opposite:
a re-export names a type, a public module hands the reader a tree to walk.
`pdfrum` is the only crate that has the first shape — 42 re-exports against 2
public modules, one of which (`pdfrum::edit`) WP7 proposes to drop. Every
other crate publishes its module tree, and eleven of the seventeen have no
`pub use` at all.
