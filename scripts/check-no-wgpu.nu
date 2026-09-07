#!/usr/bin/env nu
# M12c's isolation check: a headless build of pdfrum resolves with zero `wgpu`.
#
# The workspace grants the GPU backend an exemption from the pure-Rust
# guarantee, and bounds it with two rules. This script is the mechanical half
# of the first one — "prove this with a committed check, not an assertion" —
# and it is deliberately a separate file from scripts/ci.nu so that a reader
# looking for the blast radius of that exemption finds one place to look.
#
# What it asserts, in the order a violation would most likely arrive:
#
#   1. The `pdfrum` facade's default feature set contains no `wgpu` and no
#      `vello` at all. This is the claim an embedder cares about: `cargo add
#      pdfrum` must not put a graphics driver in their tree.
#   2. `pdfrum-tool`'s default features likewise, because the CLI is what a
#      headless or CI build actually runs.
#   3. No crate in the workspace *except* pdfrum-raster-vello depends on
#      `vello` or `wgpu` — every member, `conformance/` and `benches/`
#      included, not just the ones under `crates/`. This is the one that
#      catches the accident the other two would eventually catch anyway — a
#      crate reaching for the GPU backend "just for a test" — at the point
#      where it is one line to undo.
#
# Run standalone, or via scripts/ci.nu which calls it.

# The crate the exemption is scoped to. Everything else must be clean.
const GPU_CRATE = 'pdfrum-raster-vello'

const GPU_STACK = [wgpu wgpu-core wgpu-hal wgpu-types vello vello_encoding vello_shaders]

# The normal-dependency closure of one crate, as a list of package names.
#
# `-e normal` excludes dev- and build-dependencies: a dev-dependency does not
# ship in a consumer's tree, and the claim here is about what an embedder
# resolves.
#
# A `cargo tree` that *fails* is not a clean tree: an unresolvable manifest
# yields no output, which every filter below would read as "depends on
# nothing" and report as ok. The bash predecessor had exactly that hole
# (`2>/dev/null … || true`). Here a non-zero exit is fatal instead, because a
# check that cannot run has not passed.
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
    let found = (deps $crate | where {|d| $d in $GPU_STACK } | sort)
    if ($found | is-empty) {
        print $"ok: ($label) resolves without wgpu"
        return true
    }
    print --stderr $"error: ($label) reaches the GPU stack:"
    $found | each {|d| print --stderr $"  ($d)" } | ignore
    print --stderr "       M12c's isolation rule: nothing in the core ring may depend"
    print --stderr $"       on ($GPU_CRATE). See CONTRIBUTING.md."
    false
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    mut ok = true

    print "==> M12c isolation: the core ring resolves without wgpu"
    if not (check-clean pdfrum 'the pdfrum facade (default features)') { $ok = false }
    if not (check-clean pdfrum-tool 'pdfrum-tool (default features)') { $ok = false }

    # Every workspace member but the GPU backend itself.
    #
    # `crates/*/` is not the whole workspace. `conformance/` and `benches/` are
    # members too, and they are the two most likely places for the accident
    # this check exists to catch — a harness reaching for the GPU backend "just
    # to compare against it" is exactly how a dev-dependency becomes a normal
    # one. They are also the members whose directory name is *not* their crate
    # name (`conformance` and `pdfrum-bench`), so the name is read from each
    # manifest rather than guessed from the path.
    print "==> M12c isolation: no other workspace crate reaches the GPU stack"
    let manifests = ((glob crates/*/Cargo.toml | sort)
        ++ ([conformance/Cargo.toml benches/Cargo.toml benches/corpus-list/Cargo.toml]
            | where {|m| $m | path exists }))
    for manifest in $manifests {
        # `open` parses the manifest as TOML, so this is the package's own
        # `[package] name` rather than the first `name =` in the file — which
        # is what the bash predecessor had to approximate with sed and a
        # comment about table ordering.
        let crate = (do --ignore-errors { open $manifest | get package.name })
        if $crate == null {
            print --stderr $"error: could not read a package name from ($manifest)"
            $ok = false
            continue
        }
        if $crate == $GPU_CRATE { continue }
        if not (check-clean $crate $crate) { $ok = false }
    }

    # And the converse, so the check cannot pass by the backend having quietly
    # stopped depending on vello — which would make every assertion above
    # vacuous.
    print "==> M12c isolation: the GPU backend does still reach vello"
    if 'vello' in (deps $GPU_CRATE) {
        print $"ok: ($GPU_CRATE) depends on vello, so the checks above are not vacuous"
    } else {
        print --stderr $"error: ($GPU_CRATE) no longer depends on vello — every check above is"
        print --stderr "       trivially true and this script is measuring nothing."
        $ok = false
    }

    if not $ok { exit 1 }
    print ""
    print "M12c isolation: green."
}
