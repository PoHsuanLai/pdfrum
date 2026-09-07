#!/usr/bin/env nu
# rustdoc coverage floor. Nightly-only (`--show-coverage`); skips with a
# note on stable. Raising a floor is a normal change; lowering one wants a
# reason in the commit.

const ITEMS_FLOOR = 100.0

# Caller-facing crates. `pdfrum-doc` / `pdfrum-render` sit below 100:
# Error variants a caller matches rather than constructs, and appearance
# internals that need a loaded font before an example means anything.
# A crate absent from this table has no examples floor.
const EXAMPLE_FLOORS = {
    pdfrum: 100.0
    pdfrum-doc: 84.0
    pdfrum-edit: 100.0
    pdfrum-form: 100.0
    pdfrum-render: 98.0
    pdfrum-text: 100.0
}

# Published, but `doc = false`: the binary shares its name with the `pdfrum`
# library and documenting both writes two crates into one `doc/pdfrum/`
# (cargo#6313).
const UNDOCUMENTED_TARGET = [pdfrum-cli]

def published []: nothing -> list<string> {
    ls crates | get name | where {|dir|
        let manifest = ($dir | path join 'Cargo.toml')
        ($manifest | path exists) and (open --raw $manifest | str contains 'publish = false' | not $in)
    } | each {|dir| $dir | path basename }
    | where {|crate| $crate not-in $UNDOCUMENTED_TARGET }
    | sort
}

# rustdoc writes the machine table to `$CARGO_TARGET_DIR/doc/<crate>.json`.
def coverage [crate: string]: nothing -> record {
    let target = ($env.CARGO_TARGET_DIR? | default 'target')
    let underscored = ($crate | str replace --all '-' '_')
    let report = ([$target doc $"($underscored).json"] | path join)
    ^cargo +nightly rustdoc -p $crate -- -Z unstable-options --show-coverage --output-format json out> /dev/null
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
        print --stderr "         install with: rustup toolchain install nightly"
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
            $ok = false
        }
        if $example_floor != null and $examples < $example_floor {
            print --stderr $"error: ($crate) has examples on ($examples)% of its items, floor is ($example_floor)%"
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
    if not $ok { exit 1 }
    print "ok: rustdoc coverage is at or above the floor on every published crate"
}
