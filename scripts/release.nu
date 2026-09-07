#!/usr/bin/env nu
# Publish the workspace to crates.io, in dependency order.
#
# A version can be uploaded once and never replaced, so the two failure modes
# worth designing against are publishing in the wrong order and publishing
# half the workspace. The order comes from `scripts/publish-order.nu`, and the
# run is resumable: a crate whose version is already on the index is skipped
# rather than failing the run, so a release interrupted midway is finished by
# running this again.
#
# Verification stays on. `cargo publish` builds each packaged crate against
# the versions it will actually resolve to on crates.io, which is the only
# check that the uploaded manifest — not the workspace one — is coherent.
#
# `--dry-run` here means "do everything except upload". Note that cargo's own
# `--dry-run` cannot see unpublished siblings, so it is not used: the check
# this performs is `cargo package`, which the release workflow runs with every
# crate visible at once.

const REGISTRY = "https://crates.io/api/v1/crates"

# Whether this exact name and version is already on crates.io.
def published [name: string, version: string]: nothing -> bool {
    let url = $"($REGISTRY)/($name)/($version)"
    let res = (http get --allow-errors --full $url | default {status: 0})
    $res.status == 200
}

def main [
    --dry-run  # Package and verify every crate, but upload nothing.
] {
    cd ($env.FILE_PWD | path dirname)

    let version = (^cargo metadata --no-deps --format-version 1
        | from json
        | get packages
        | where name == "pdfrum"
        | first
        | get version)

    let order = (^./scripts/publish-order.nu | lines | where {|l| $l != "" })

    print $"==> pdfrum ($version): ($order | length) crates"
    if $dry_run {
        print "    dry run: packaging and verifying, uploading nothing"
    }
    print ""

    if $dry_run {
        # One `cargo package` over the whole set, so each crate resolves its
        # unpublished siblings from the workspace instead of the index. Run
        # per-crate this would fail on the first internal dependency.
        let excluded = (^cargo metadata --no-deps --format-version 1
            | from json
            | get packages
            | where {|p| $p.publish == [] }
            | get name)
        let flags = ($excluded | each {|n| [--exclude $n] } | flatten)
        ^cargo package --workspace --locked ...$flags
        print ""
        print $"packaged ($order | length) crates."
        return
    }

    for name in $order {
        if (published $name $version) {
            print $"    ($name) ($version) is already published; skipping"
            continue
        }
        print $"==> publishing ($name) ($version)"
        ^cargo publish -p $name --locked

        # crates.io serves the index through a cache, so the next crate can
        # ask for this one before it is visible. Wait for it rather than
        # racing, and give up loudly instead of publishing out of order.
        print $"    waiting for ($name) ($version) on the index"
        mut waited = 0
        while not (published $name $version) {
            if $waited >= 300 {
                print --stderr $"error: ($name) ($version) did not appear on the index within 300s"
                print --stderr "       re-run this script once it does; published crates are skipped"
                exit 1
            }
            sleep 5sec
            $waited = $waited + 5
        }
    }

    print ""
    print $"published pdfrum ($version)."
}
