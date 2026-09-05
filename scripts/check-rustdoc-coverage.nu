#!/usr/bin/env nu
# The rustdoc floor: every published crate documents all of its items, and the
# crates a library caller actually touches carry worked examples.
#
# Two numbers, two different failure modes. **Documented items** is the one
# `missing_docs` already warns about for public items — but rustdoc counts
# private items and struct fields too, which `missing_docs` does not, so a
# field added to a public enum variant slips past the lint and is caught here.
# **Examples** is the number no lint watches at all: a type can be fully
# documented and still leave a caller with no idea what to type. It regresses
# silently, one new method at a time, which is why it gets a floor rather than
# a target.
#
# The floors below are the levels reached when this check was written. They are
# a ratchet: raising one is a normal change, lowering one is a decision that
# should be argued for in the commit message.
#
# `--show-coverage` is a nightly-only rustdoc flag (`-Z unstable-options`).
# That is a *tooling* requirement and not a crack in the pure-Rust/stable
# story — nothing here is compiled into the library, and the ordinary gate
# still passes on stable with this step printing a note and standing down, the
# same bargain `cargo deny`, the C test and the WebAssembly tests get in
# scripts/ci.nu. CI itself has nightly, so the floor is enforced there.

# Documented-items floor: 100% on every published crate, no exceptions.
const ITEMS_FLOOR = 100.0

# Examples floor, per crate: the six a library caller actually touches, each
# held at the level it reached rather than at a round number below it, so the
# ratchet keeps the gain instead of leaving slack to lose.
#
# `pdfrum-doc` and `pdfrum-render` sit below 100 for a reason rather than for
# want of effort. Both keep a handful of items that cannot carry a standalone
# example: an `Error` enum, whose variants a caller matches rather than
# constructs; `pub(crate)` and private-module items no doctest can name; and
# `pdfrum-doc`'s appearance-generation internals, which need a loaded font and
# a widget dictionary carrying `/DA`, `/MK` and `/AP` before an assertion means
# anything — a page of fixture setup proving nothing a caller would recognise.
# The types a caller of those crates starts from are all covered.
#
# Absent from this table means no examples floor — the crate is internal
# plumbing or a thin backend shim whose types a caller names but does not drive.
const EXAMPLE_FLOORS = {
    pdfrum: 100.0
    pdfrum-doc: 84.0
    pdfrum-edit: 100.0
    pdfrum-form: 100.0
    pdfrum-render: 98.0
    pdfrum-text: 100.0
}

# Every crate whose `Cargo.toml` does not say `publish = false`.
#
# `pdfrum-cli` is published and is deliberately not in the list: its only
# target is a binary carrying `doc = false`, because the binary shares its name
# with the `pdfrum` library and documenting both writes two crates into one
# `doc/pdfrum/` (cargo#6313). It produces no rustdoc, so there is nothing here
# to measure — its `//` and `//!` prose is still read by `cargo doc`'s warning
# gate in scripts/ci.nu.
const UNDOCUMENTED_TARGET = [pdfrum-cli]

def published []: nothing -> list<string> {
    ls crates | get name | where {|dir|
        let manifest = ($dir | path join 'Cargo.toml')
        ($manifest | path exists) and (open --raw $manifest | str contains 'publish = false' | not $in)
    } | each {|dir| $dir | path basename }
    | where {|crate| $crate not-in $UNDOCUMENTED_TARGET }
    | sort
}

# One crate's coverage, read out of the report rustdoc writes to a file.
# `--show-coverage` prints the human table to stdout but writes the machine
# one to `$CARGO_TARGET_DIR/doc/<crate>.json`, and that is the one to parse:
# the table is formatted for a terminal and its columns move.
def coverage [crate: string]: nothing -> record {
    let target = ($env.CARGO_TARGET_DIR? | default 'target')
    let underscored = ($crate | str replace --all '-' '_')
    let report = ([$target doc $"($underscored).json"] | path join)

    # Only stdout is dropped — that is the human table, which this reads out
    # of the JSON instead. Anything rustdoc says on stderr is a real build
    # problem and must reach the terminal, and a non-zero exit stops the
    # script the way every other external call in scripts/ci.nu does.
    ^cargo +nightly rustdoc -p $crate -- -Z unstable-options --show-coverage --output-format json out> /dev/null

    # The JSON is one entry per source file; the crate's number is the sum.
    let files = (open $report | values)
    let total = ($files | get total | math sum)
    let with_docs = ($files | get with_docs | math sum)
    let total_examples = ($files | get total_examples | math sum)
    let with_examples = ($files | get with_examples | math sum)

    {
        items: (if $total == 0 { 100.0 } else { $with_docs * 100.0 / $total })
        examples: (if $total_examples == 0 { 100.0 } else { $with_examples * 100.0 / $total_examples })
    }
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    let has_nightly = (^rustup toolchain list | lines | any {|t| $t | str starts-with 'nightly' })
    if not $has_nightly {
        print --stderr "warning: no nightly toolchain; skipping the rustdoc coverage floor"
        print --stderr "         `--show-coverage` is nightly-only tooling, not a build"
        print --stderr "         dependency — install with: rustup toolchain install nightly"
        return
    }

    mut ok = true
    mut rows = []

    for crate in (published) {
        let measured = (coverage $crate)
        let example_floor = ($EXAMPLE_FLOORS | get --optional $crate)
        let items = ($measured.items | math round --precision 1)
        let examples = ($measured.examples | math round --precision 1)

        if $items < $ITEMS_FLOOR {
            print --stderr $"error: ($crate) documents ($items)% of its items, floor is ($ITEMS_FLOOR)%"
            print --stderr "       rustdoc counts private items and struct fields, which"
            print --stderr "       `missing_docs` does not — look for an undocumented field."
            $ok = false
        }

        if $example_floor != null and $examples < $example_floor {
            print --stderr $"error: ($crate) has examples on ($examples)% of its items, floor is ($example_floor)%"
            print --stderr "       a caller-facing crate; add a doctest to the new item."
            $ok = false
        }

        $rows = ($rows | append {
            crate: $crate
            items: $"($items)%"
            examples: $"($examples)%"
            floor: (if $example_floor == null { '-' } else { $"($example_floor)%" })
        })
    }

    print ($rows | table)

    if not $ok {
        print --stderr ""
        print --stderr "The floors live in scripts/check-rustdoc-coverage.nu. Raising one is a"
        print --stderr "normal change; lowering one wants a reason in the commit message."
        exit 1
    }

    print "ok: rustdoc coverage is at or above the floor on every published crate"
}
