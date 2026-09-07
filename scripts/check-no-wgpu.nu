#!/usr/bin/env nu
# Headless pdfrum resolves with zero wgpu. Called from scripts/ci.nu.

const GPU_CRATE = 'pdfrum-raster-vello'
const GPU_STACK = [wgpu wgpu-core wgpu-hal wgpu-types vello vello_encoding vello_shaders]

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
    let found = (deps $crate | where {|d| $d in $GPU_STACK } | sort)
    if ($found | is-empty) {
        print $"ok: ($label) resolves without wgpu"
        return true
    }
    print --stderr $"error: ($label) reaches the GPU stack:"
    $found | each {|d| print --stderr $"  ($d)" } | ignore
    print --stderr "       isolation rule: nothing in the core ring may depend"
    print --stderr $"       on ($GPU_CRATE). See CONTRIBUTING.md."
    false
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    mut ok = true

    print "==> GPU isolation: the core ring resolves without wgpu"
    if not (check-clean pdfrum 'the pdfrum facade (default features)') { $ok = false }
    if not (check-clean pdfrum-tool 'pdfrum-tool (default features)') { $ok = false }

    print "==> GPU isolation: no other workspace crate reaches the GPU stack"
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
        if $crate == $GPU_CRATE { continue }
        if not (check-clean $crate $crate) { $ok = false }
    }

    print "==> GPU isolation: the GPU backend does still reach vello"
    if 'vello' in (deps $GPU_CRATE) {
        print $"ok: ($GPU_CRATE) depends on vello, so the checks above are not vacuous"
    } else {
        print --stderr $"error: ($GPU_CRATE) no longer depends on vello — every check above is"
        print --stderr "       trivially true and this script is measuring nothing."
        $ok = false
    }

    if not $ok { exit 1 }
    print ""
    print "GPU isolation: green."
}
