#!/usr/bin/env nu
# Default pdfrum resolves with zero boa. Called from scripts/ci.nu.

const BOA_CRATES = [boa_engine boa_ast boa_parser boa_gc boa_interner boa_string boa_macros]

# `-e normal` only. A failed `cargo tree` is not a clean tree.
def deps [crate: string, extra: list<string> = []]: nothing -> list<string> {
    let r = (^cargo tree -e normal -p $crate ...$extra --prefix none | complete)
    if $r.exit_code != 0 {
        print --stderr $"error: `cargo tree -p ($crate)` failed; the tree could not be"
        print --stderr "       resolved, so this check has not run. cargo said:"
        $r.stderr | lines | each {|l| print --stderr $"  ($l)" } | ignore
        exit 1
    }
    $r.stdout | lines | split column ' ' name | get name | uniq
}

def check-clean [crate: string, label: string]: nothing -> bool {
    let found = (deps $crate | where {|d| $d in $BOA_CRATES } | sort)
    if ($found | is-empty) {
        print $"ok: ($label) resolves without boa"
        return true
    }
    print --stderr $"error: ($label) reaches the JavaScript engine:"
    $found | each {|d| print --stderr $"  ($d)" } | ignore
    print --stderr "       isolation rule: no crate's DEFAULT features may"
    print --stderr "       depend on boa. See CONTRIBUTING.md."
    false
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    mut ok = true

    print "==> boa isolation: the default tree resolves without boa"
    if not (check-clean pdfrum 'the pdfrum facade (default features)') { $ok = false }
    if not (check-clean pdfrum-tool 'pdfrum-tool (default features)') { $ok = false }

    print "==> boa isolation: no workspace crate's default features reach boa"
    let manifests = ((glob crates/*/Cargo.toml | sort)
        ++ ([conformance/Cargo.toml benches/Cargo.toml benches/corpus-list/Cargo.toml]
            | where {|m| $m | path exists }))
    for manifest in $manifests {
        let crate = (do --ignore-errors { open $manifest | get package.name })
        if $crate == null {
            print --stderr $"error: could not read a package name from ($manifest)"
            $ok = false
            continue
        }
        if not (check-clean $crate $crate) { $ok = false }
    }

    print "==> boa isolation: the javascript feature does still bring boa"
    for probe in [
        {crate: 'pdfrum-form', why: "the engine's own crate"}
        {crate: 'pdfrum', why: "the facade's forwarding feature"}
    ] {
        if 'boa_engine' in (deps $probe.crate [--features javascript]) {
            print $"ok: ($probe.crate) --features javascript depends on boa_engine — ($probe.why)"
        } else {
            print --stderr $"error: ($probe.crate) --features javascript no longer reaches boa_engine —"
            print --stderr "       every check above is trivially true and this script is"
            print --stderr "       measuring nothing."
            $ok = false
        }
    }

    if not $ok { exit 1 }
    print ""
    print "boa isolation: green."
}
