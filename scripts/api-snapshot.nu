#!/usr/bin/env nu
# Public-API baseline: gate the working tree against docs/api-baseline/.
#
# /scripts/api-snapshot.nu # same as `diff`
# /scripts/api-snapshot.nu diff # print the delta; exit 1 on drift
# /scripts/api-snapshot.nu check # CI gate
# /scripts/api-snapshot.nu update # regenerate the committed baseline
# /scripts/api-snapshot.nu list # per-crate item counts
#
# Requires `cargo public-api` and nightly
# cargo install cargo-public-api --locked
#
# C header is scripts/capi-header.nu.

const BASELINE = 'docs/api-baseline'
const TOOLCHAIN = 'nightly'
# Omit blanket / auto-trait / derived impls.
const SIMPLIFY = '-sss'

# Published library crates. Skips `publish = false` and crates with no lib.
def published-libs []: nothing -> list<string> {
    glob crates/*/Cargo.toml
    | sort
    | each {|manifest|
        let m = (open $manifest)
        let published = (($m.package.publish? | default true) != false)
        let has_lib = (($manifest | path dirname | path join src lib.rs) | path exists)
        if $published and $has_lib { $m.package.name } else { null }
    }
    | compact
}

# Non-default surfaces. Named, not derived.
const FEATURED = [
    {file: 'pdfrum+javascript', crate: 'pdfrum', features: 'javascript'}
    {file: 'pdfrum+png', crate: 'pdfrum', features: 'png'}
    {file: 'pdfrum+markdown', crate: 'pdfrum', features: 'markdown'}
    {file: 'pdfrum+svg-export', crate: 'pdfrum', features: 'svg-export'}
    {file: 'pdfrum+svg-import', crate: 'pdfrum', features: 'svg-import'}
    {file: 'pdfrum+svg-text', crate: 'pdfrum', features: 'svg-text'}
    {file: 'pdfrum+tiny-skia+agg', crate: 'pdfrum', features: 'tiny-skia,agg'}
    {file: 'pdfrum-page+codecs', crate: 'pdfrum-page', features: 'jpeg2000,jbig2,ccitt'}
    {file: 'pdfrum-filters+ccitt', crate: 'pdfrum-filters', features: 'ccitt'}
    {file: 'pdfrum-font+system-fonts', crate: 'pdfrum-font', features: 'system-fonts'}
    {file: 'pdfrum-render+png', crate: 'pdfrum-render', features: 'png'}
]

def surface [crate: string, features: string = '']: nothing -> string {
    let flags = (if ($features | is-empty) { [] } else { [--features $features] })
    let r = (^cargo $"+($TOOLCHAIN)" public-api $SIMPLIFY -p $crate ...$flags | complete)
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
        print --stderr "       It is developer tooling only — do not add it to any crate's Cargo.toml"
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
    for extra in $FEATURED {
        let path = ($BASELINE | path join $"($extra.file).txt")
        surface $extra.crate $extra.features | save --force $path
        print $"  ($extra.file)"
    }
    let total = (($crates | length) + ($FEATURED | length))
    print ""
    print $"Wrote ($total) files to ($BASELINE)/."
    print "Review the diff before committing: this is a baseline, and a change to"
    print "it is a change to what `cargo add pdfrum` sees."
}

# Set difference both ways. `cargo public-api` emits its items in a stable
# sort, so a plain line-set comparison is exactly the API delta — no
# reordering noise to filter out, which is why this can be structured data
# rather than a shell out to `diff`.
def line-delta [now: list<string>, then: list<string>]: nothing -> record {
    {
        added: ($now | where {|l| $l not-in $then })
        removed: ($then | where {|l| $l not-in $now })
    }
}

# One crate's committed snapshot vs the working tree, as structured data.
def snapshot-delta [target: record]: nothing -> record {
    let path = ($BASELINE | path join $"($target.file).txt")
    let now = (surface $target.crate $target.features | lines)
    let then = (if ($path | path exists) { open --raw $path | lines } else { [] })
    (line-delta $now $then) | merge {
        crate: $target.file
        now: $now
        then: $then
        missing_baseline: (not ($path | path exists))
    }
}

def snapshot-targets []: nothing -> list<record> {
    (
        (published-libs | each {|crate| {file: $crate, crate: $crate, features: ''} })
        ++ $FEATURED
    )
}

def print-delta [r: record] {
    print ""
    if $r.missing_baseline {
        print $"($r.crate): no committed baseline — new crate?"
    } else {
        print $"($r.crate): +($r.added | length) -($r.removed | length)"
    }
    $r.removed | each {|l| print $"  - ($l)" } | ignore
    $r.added   | each {|l| print $"  + ($l)" } | ignore
}

# Diff the working tree's surface against the committed baseline.
#
# Exits non-zero when they differ. `check` is the CI spelling of the same
# comparison, with the failure message a developer acting on a red gate needs.
def "main diff" [] {
    cd ($env.FILE_PWD | path dirname)
    require-tool

    let targets = (snapshot-targets)
    print $"==> diffing ($targets | length) surfaces against the committed baseline"

    # Collected rather than printed as they go, so the summary can lead with
    # the count and a reader knows how much scrolling is ahead of them.
    let results = ($targets | each {|target| snapshot-delta $target })
    let changed = ($results | where {|r| ($r.added | is-not-empty) or ($r.removed | is-not-empty) })

    for r in $changed { print-delta $r }

    print ""
    if ($changed | is-empty) {
        print "The public API matches the committed baseline."
        return
    }
    print $"($changed | length) of ($targets | length) surfaces differ from the baseline."
    print "Review the diff. A change to the baseline is a change to what"
    print "`cargo add pdfrum` sees. If this is intended, re-record it with"
    print "  ./scripts/api-snapshot.nu update"
    print "deliberately, and say in the commit message why the surface moved."
    exit 1
}

# The CI gate. Same comparison as `diff`, then a non-vacuity proof that the
# set-difference still reports a planted extra item — so a comparison that
# stopped looking cannot pass by matching an empty delta.
def "main check" [] {
    cd ($env.FILE_PWD | path dirname)
    require-tool

    let targets = (snapshot-targets)
    print $"==> public-API snapshot: ($targets | length) surfaces must match the baseline"

    let results = ($targets | each {|target| snapshot-delta $target })
    let changed = ($results | where {|r| ($r.added | is-not-empty) or ($r.removed | is-not-empty) })

    for r in $changed { print-delta $r }

    if not ($changed | is-empty) {
        print ""
        print --stderr $"error: ($changed | length) of ($targets | length) surfaces differ from the committed baseline."
        print --stderr "       Review the diff above. A change to docs/api-baseline/"
        print --stderr "       is a change to what `cargo add pdfrum` sees. If this is"
        print --stderr "       intended, re-record it deliberately with"
        print --stderr "         ./scripts/api-snapshot.nu update"
        exit 1
    }
    print "ok: the public API matches the committed baseline"

    # The converse, so the check cannot pass by having stopped looking.
    # A phantom item in the committed set must show up as a removal against
    # the working tree we just measured — one cargo-public-api run, not two.
    print "==> the snapshot comparison does still see a planted item"
    let probe = 'pub fn pdfrum::planted_wp13_probe()'
    let planted = ($results | each {|r|
        (line-delta $r.now ($r.then | append $probe)).removed
    } | flatten)
    if $probe not-in $planted {
        print --stderr "error: a planted public item was not reported as drift —"
        print --stderr "       the comparison above is measuring nothing."
        exit 1
    }
    print "ok: a planted public item is reported as drift"

    print ""
    print "public-API snapshot: green."
}

def "main list" [] {
    cd ($env.FILE_PWD | path dirname)

    published-libs | each {|crate|
        let path = ($BASELINE | path join $"($crate).txt")
        if not ($path | path exists) { return null }
        let lines = (open --raw $path | lines)
        {
            crate: $crate
            items: ($lines | length)
            'pub mod': ($lines | where {|l| $l | str starts-with 'pub mod ' } | length)
            'pub use': ($lines | where {|l| $l | str starts-with 'pub use ' } | length)
        }
    } | compact
}

def main [] {
    main diff
}
