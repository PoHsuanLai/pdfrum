#!/usr/bin/env nu
# The order `scripts/release.nu` publishes in, derived rather than listed.
#
# A crate may only reach crates.io after every crate it depends on is already
# there, so the order is a topological sort of the workspace's internal
# `[dependencies]` and `[build-dependencies]` edges. Dev-dependency edges are
# left out on purpose: they form cycles here (the leaf crates test through the
# `pdfrum` facade, which depends on them), and cargo strips a path-only
# dev-dependency from the manifest it uploads, so those edges do not constrain
# publication.
#
# `publish = false` members — the C ABI, the WebAssembly binding, the internal
# tool, the benches and the corpus list — are dropped, not sorted.

def main [] {
    let meta = (^cargo metadata --no-deps --format-version 1 | from json)
    let members = ($meta.packages | where {|p| $p.publish != [] })
    let names = ($members | get name)

    mut order = []
    mut placed = []
    mut remaining = $names

    # Kahn's algorithm, taking the alphabetically first ready crate each round
    # so the same workspace always prints the same order.
    while not ($remaining | is-empty) {
        let done = $placed
        let ready = ($remaining | where {|n|
            let pkg = ($members | where name == $n | first)
            let deps = ($pkg.dependencies
                | where {|d| $d.kind == null or $d.kind == "build" }
                | get name
                | where {|d| $d in $names and $d != $n })
            ($deps | all {|d| $d in $done })
        } | sort)

        if ($ready | is-empty) {
            print --stderr "error: a dependency cycle blocks the publish order:"
            $remaining | sort | each {|n| print --stderr $"  ($n)" } | ignore
            exit 1
        }

        let next = ($ready | first)
        $order = ($order | append $next)
        $placed = ($placed | append $next)
        $remaining = ($remaining | where {|n| $n != $next })
    }

    $order | each {|n| print $n } | ignore
}
