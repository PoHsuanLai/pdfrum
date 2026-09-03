# The public-API baseline

**Status:** the public-API gate, as of WP13 (2026-09-03).
`./scripts/api-snapshot.nu check` is wired into `scripts/ci.nu`. A drift is a
red run. These files began as a "before" picture, taken 2026-09-02 at commit
`9b8f74b`; they are now the surface `cargo add` is held to.

One file per published library crate, each holding that crate's complete
public API as `cargo public-api` prints it. `pdfrum.txt` is the facade — the
surface a `cargo add pdfrum` caller sees — and the rest are the member crates
it composes. One file is not a crate: `pdfrum+javascript.txt` is the facade again
with its one cargo feature on (WP12), and the reason it exists is under "What
else is not here" below.

---

## Why this exists, and why it is now a gate

[`docs/design/idiomatic-api.md`](../../design/idiomatic-api.md) planned
thirteen work packages that break the public surface **on purpose**. Its
sequence put `cargo public-api` snapshotting last, as WP13, and called it a
drift gate — and held the check *out* of `scripts/ci.nu` for the length of
the pass, because a drift check during those packages would have reddened
every intentional break.

The files were taken first, at `9b8f74b`, so each package could measure
itself against a surface that had not already moved. That measurement is
done. WP13 (2026-09-03) turned the same command into the gate: `scripts/ci.nu`
runs `./scripts/api-snapshot.nu check` after `cargo doc`. A change to these
files is a change to what `cargo add pdfrum` sees.

A line here is not an endorsement of the original surface. Targets the pass
hit are simply gone — `FormSession::on_button(…, down: bool, …)` is what
that looks like once it is deleted from `pdfrum.txt`.

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
| features | default, plus one recorded second surface (see below) |

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
- **Non-default features, with one exception.** The default surface is what
  `cargo add` gets, and so is what these files record.

  *Amended 2026-09-02 (WP12).* One feature earns a second file:
  **`pdfrum+javascript.txt`** is `pdfrum --features javascript`, nine items more than
  `pdfrum.txt` (`ScriptCascade`, `ScriptConfig`, `TranscriptLine`,
  `ScriptBuildError`, `ScriptFailure`, `ScriptStop`, `FieldActions`,
  `FormSession::with_scripts` and `FormSession::scripts`). The test for
  earning one is whether the crate's own documentation tells an embedder to
  turn the feature on: the facade's crate docs carry a `# Features` section
  naming `script`, so its items are part of the product and a change to them
  would otherwise be invisible to every gate here.

  `pdfrum-render/profiling` does not qualify — it is a profiling switch,
  not a surface offered to callers. Neither does `pdfrum-form/javascript`, whose
  items are the same ones `pdfrum+javascript.txt` records one layer up; recording
  both would make one API change diff in two files. `scripts/api-snapshot.nu`
  names the list in a `FEATURED` constant rather than deriving it, so a
  feature joining it is a decision someone makes rather than a consequence of
  a manifest edit.

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
./scripts/api-snapshot.nu            # same as `diff`
./scripts/api-snapshot.nu diff       # print the delta; exit 1 on drift
./scripts/api-snapshot.nu check      # the CI gate (drift is a failure)
./scripts/api-snapshot.nu update     # rewrite these files from the working tree
./scripts/api-snapshot.nu list       # the per-crate item counts, as a table
```

`diff` and `check` exit non-zero when the surface has moved, and print the
added and removed items per crate. That output is the review.

`update` rewrites every file. Read its `git diff` before committing — a change
here is a change to what `cargo add pdfrum` sees — and say in the commit
message why the surface moved. Do not bless a drift because CI is red; bless
it because the new item is the API.

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
