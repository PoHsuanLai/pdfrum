#!/usr/bin/env nu
# The public-API baseline: regenerate docs/status/api-baseline/, or diff the
# working tree's surface against what is committed there.
#
# **This is deliberately NOT a gate, and must not be added to scripts/ci.nu.**
#
# docs/design/idiomatic-api.md plans thirteen work packages that break the
# public surface *on purpose*, and its §7 sequence puts the `cargo public-api`
# gate last, as WP13. That ordering is right for a gate and wrong for a
# baseline: a drift check wired in today would turn every intentional WP break
# into a red CI run, and the pass would spend its life re-blessing the file it
# is supposed to be changing. So this script exists so the pass can *measure*
# itself — run `diff` before and after a package and the output is that
# package's actual blast radius — and WP13 is where the same command becomes a
# `check` subcommand someone wires into ci.nu.
#
# The other half of the argument is why the baseline had to be taken *first*.
# WP13 snapshots "the surface we meant to keep". That is the after picture. If
# nobody records the before, each package's diff is against a surface that has
# already moved under it and no one can say what a given package changed. The
# committed files under docs/status/api-baseline/ are that before picture: the
# surface as it stood at 9b8f74b, the commit that added the design document.
# They are a measurement, not an aspiration — most of what they record is what
# the thirteen packages exist to remove.
#
# Usage:
#
#   ./scripts/api-snapshot.nu            # diff working tree vs committed baseline
#   ./scripts/api-snapshot.nu update     # regenerate the committed baseline
#   ./scripts/api-snapshot.nu list       # print the per-crate item counts
#
# Requires `cargo public-api` and a nightly toolchain, because the tool reads
# rustdoc's JSON output and that is nightly-only. Neither is a dependency of
# this workspace: `cargo public-api` is a `~/.cargo/bin` binary, it is NOT in
# DEPS.md, and it must never appear in a crate's Cargo.toml. Install with
#
#   cargo install cargo-public-api --locked
#
# See docs/status/api-baseline/README.md for what the files contain.

# Where the committed snapshots live, relative to the repo root.
const BASELINE = 'docs/status/api-baseline'

# The toolchain rustdoc JSON needs. Pinned to the channel, not a date: the
# output format is stable enough across nightlies that a date pin would cost
# more in churn than it buys in reproducibility, and README.md records the
# exact rustc that produced the committed files.
const TOOLCHAIN = 'nightly'

# `-sss` is `--omit blanket-impls,auto-trait-impls,auto-derived-impls`.
#
# Without it roughly half of every file is `impl<T, U> Into<U> for T`,
# `impl Send for ...` and derived `Clone`/`Debug`/`PartialEq` — noise that is
# identical for every type and tells a reviewer nothing about the surface.
# With it the file is the API a reader would write down by hand, which is what
# STYLE.md §4's "one screen a reviewer can read" is measured against.
#
# The trade: an auto-trait regression (a type quietly losing `Send`) does not
# show in the diff. The facade already has a unit test asserting `Send + Sync`
# on its public types — that is the check for that property, and it is a better
# one than a 400-line diff nobody reads.
const SIMPLIFY = '-sss'

# Every workspace member that publishes a library, and is therefore a surface
# some `cargo add` reaches.
#
# Read from the manifests rather than listed here, on the same reasoning as
# check-no-boa.nu: a crate added to the workspace should appear in the baseline
# without anyone remembering to edit this file. Two kinds of member are
# excluded, and both exclusions are recorded in the README so a reader does not
# have to infer them from a missing file:
#
#   - `publish = false` (pdfrum-raster-vello, pdfrum-script). Nothing can
#     `cargo add` them, so they have no public API in the sense this measures.
#     docs/design/idiomatic-api.md §4 rules that the *published* members stay
#     published; these two were never in that set.
#   - No library target (pdfrum-tool). A binary has no public API at all;
#     `cargo public-api` fails on it rather than emitting an empty file.
#
# `conformance/` and `benches/` are workspace members too, and are excluded for
# both reasons at once.
def published-libs []: nothing -> list<string> {
    glob crates/*/Cargo.toml
    | sort
    | each {|manifest|
        let m = (open $manifest)
        # `publish` absent means published; `publish = false` means not.
        let published = (($m.package.publish? | default true) != false)
        # A crate with src/lib.rs has a library target. Cargo's autodiscovery
        # makes this exact for every crate in this workspace; a manifest that
        # named a `[lib] path` elsewhere would need reading, and none does.
        let has_lib = (($manifest | path dirname | path join src lib.rs) | path exists)
        if $published and $has_lib { $m.package.name } else { null }
    }
    | compact
}

# One crate's public API, as `cargo public-api` prints it.
#
# A failure here is fatal rather than an empty file. The whole point of a
# baseline is that a missing surface is loud: a crate that stopped building
# would otherwise diff as "every public item removed", which reads as a
# spectacular API break rather than as a broken build.
def surface [crate: string]: nothing -> string {
    let r = (^cargo $"+($TOOLCHAIN)" public-api $SIMPLIFY -p $crate | complete)
    if $r.exit_code != 0 {
        print --stderr $"error: `cargo public-api -p ($crate)` failed:"
        $r.stderr | lines | each {|l| print --stderr $"  ($l)" } | ignore
        print --stderr ""
        print --stderr $"       This is fatal: a crate that will not build has no"
        print --stderr $"       measurable surface, and writing an empty file would"
        print --stderr $"       diff as though every public item had been removed."
        exit 1
    }
    $r.stdout
}

def require-tool []: nothing -> nothing {
    if (which cargo-public-api | is-empty) {
        print --stderr "error: cargo-public-api is not installed."
        print --stderr "       install with: cargo install cargo-public-api --locked"
        print --stderr "       It is developer tooling only — do not add it to DEPS.md"
        print --stderr "       or to any crate's Cargo.toml."
        exit 1
    }
    let toolchains = (^rustup toolchain list | lines)
    if ($toolchains | where {|t| $t | str starts-with $TOOLCHAIN } | is-empty) {
        print --stderr $"error: the ($TOOLCHAIN) toolchain is not installed, and rustdoc"
        print --stderr $"       JSON — which cargo-public-api reads — is nightly-only."
        print --stderr $"       install with: rustup toolchain install ($TOOLCHAIN)"
        exit 1
    }
}

# Regenerate every committed snapshot.
def "main update" [] {
    cd ($env.FILE_PWD | path dirname)
    require-tool
    mkdir $BASELINE

    let crates = (published-libs)
    print $"==> regenerating the API baseline for ($crates | length) published library crates"
    for crate in $crates {
        let path = ($BASELINE | path join $"($crate).txt")
        surface $crate | save --force $path
        print $"  ($crate)"
    }
    print ""
    print $"Wrote ($crates | length) files to ($BASELINE)/."
    print "Review the diff before committing: this is a baseline, and a change to"
    print "it is a change to what `cargo add pdfrum` sees."
}

# Diff the working tree's surface against the committed baseline.
#
# Exits non-zero when they differ — so a caller who *wants* a gate can have one
# — but nothing in scripts/ci.nu calls this, and per the header nothing should
# until WP13.
def "main diff" [] {
    cd ($env.FILE_PWD | path dirname)
    require-tool

    let crates = (published-libs)
    print $"==> diffing ($crates | length) crates against the committed baseline"

    # Collected rather than printed as they go, so the summary can lead with
    # the count and a reader knows how much scrolling is ahead of them.
    let results = ($crates | each {|crate|
        let path = ($BASELINE | path join $"($crate).txt")
        let now = (surface $crate | lines)
        let then = (if ($path | path exists) { open --raw $path | lines } else { [] })
        {
            crate: $crate
            missing_baseline: (not ($path | path exists))
            # Set difference both ways. `cargo public-api` emits its items in a
            # stable sort, so a plain line-set comparison is exactly the API
            # delta — no reordering noise to filter out, which is why this can
            # be structured data rather than a shell out to `diff`.
            added: ($now | where {|l| $l not-in $then })
            removed: ($then | where {|l| $l not-in $now })
        }
    })

    let changed = ($results | where {|r| ($r.added | is-not-empty) or ($r.removed | is-not-empty) })

    for r in $changed {
        print ""
        if $r.missing_baseline {
            print $"($r.crate): no committed baseline — new crate?"
        } else {
            print $"($r.crate): +($r.added | length) -($r.removed | length)"
        }
        $r.removed | each {|l| print $"  - ($l)" } | ignore
        $r.added   | each {|l| print $"  + ($l)" } | ignore
    }

    print ""
    if ($changed | is-empty) {
        print "The public API matches the committed baseline."
        return
    }
    print $"($changed | length) of ($crates | length) crates differ from the baseline."
    print "If this is an intended idiomatic-API work package, re-record it with"
    print "  ./scripts/api-snapshot.nu update"
    print "and say in the commit message which WP the diff is."
    exit 1
}

# The per-crate item counts, as a table.
#
# This is the number STYLE.md §4's "fits in one lib.rs re-export block a
# reviewer can read in one screen" is actually about, and the reason it is here
# rather than in a comment somewhere: the claim is checkable, and WP11 is
# graded against it crate by crate.
def "main list" [] {
    cd ($env.FILE_PWD | path dirname)

    published-libs | each {|crate|
        let path = ($BASELINE | path join $"($crate).txt")
        if not ($path | path exists) { return null }
        let lines = (open --raw $path | lines)
        {
            crate: $crate
            items: ($lines | length)
            # `pub mod` is the WP11 tell: a curated crate re-exports types and
            # keeps its modules private, so every `pub mod` past the crate root
            # is a module tree a reviewer has to read instead of a name.
            'pub mod': ($lines | where {|l| $l | str starts-with 'pub mod ' } | length)
            'pub use': ($lines | where {|l| $l | str starts-with 'pub use ' } | length)
        }
    } | compact
}

def main [] {
    main diff
}
