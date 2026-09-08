#!/usr/bin/env nu
# Cut CHANGELOG.md and, if asked, bump the workspace version.
#
# A release is a commit that already contains the version it claims: the tag
# and `cargo publish` both read that commit, so this script is the thing that
# runs *before* the tag. It turns `[Unreleased]` into `[x.y.z] - date`, leaves
# a fresh `[Unreleased]`, and writes the same version into `Cargo.toml` when
# the argument differs from `[workspace.package]`.
#
#   ./scripts/prepare-release.nu           # cut for the version already in Cargo.toml
#   ./scripts/prepare-release.nu 0.1.1     # bump, then cut
#   ./scripts/prepare-release.nu --notes   # print the GitHub Release body; no edits
#
# It does not commit, tag, or publish. Commit the result, open a PR, merge to
# main. `.github/workflows/tag-release.yml` tags `v*` once CI is green and
# calls the publish workflow, which also opens the GitHub Release from
# `--notes`.

def workspace-version []: nothing -> string {
    open Cargo.toml | get workspace.package.version
}

def repo-url []: nothing -> string {
    open Cargo.toml | get workspace.package.repository
}

def parse-semver [v: string] {
    let m = ($v | parse --regex '^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$')
    if ($m | is-empty) {
        print --stderr $"error: ($v) is not a version of the form x.y.z"
        exit 1
    }
    {
        major: ($m.0.capture0 | into int)
        minor: ($m.0.capture1 | into int)
        patch: ($m.0.capture2 | into int)
    }
}

def ver-cmp [a: string, b: string]: nothing -> int {
    let pa = parse-semver $a
    let pb = parse-semver $b
    if $pa.major != $pb.major { return ($pa.major - $pb.major) }
    if $pa.minor != $pb.minor { return ($pa.minor - $pb.minor) }
    $pa.patch - $pb.patch
}

def has-section [text: string, version: string]: nothing -> bool {
    let heading = $"## [($version)]"
    $text | lines | any {|l| $l | str starts-with $heading }
}

def trim-blank [lines: list<string>]: nothing -> list<string> {
    mut start = 0
    mut end = ($lines | length)
    while $start < $end and ($lines | get $start | str trim | is-empty) {
        $start = $start + 1
    }
    while $end > $start and ($lines | get ($end - 1) | str trim | is-empty) {
        $end = $end - 1
    }
    $lines | skip $start | take ($end - $start)
}

def drop-link-footer [lines: list<string>]: nothing -> list<string> {
    mut end = ($lines | length)
    while $end > 0 {
        let line = $lines | get ($end - 1)
        if ($line | str trim | is-empty) or ($line =~ '^\[.+\]: https?://') {
            $end = $end - 1
        } else {
            break
        }
    }
    $lines | take $end
}

def version-headings [lines: list<string>]: nothing -> list<string> {
    $lines
    | parse --regex '^## \[(\d+\.\d+\.\d[^]]*)\]'
    | get capture0
}

def make-links [versions: list<string>, repo: string]: nothing -> list<string> {
    if ($versions | is-empty) {
        return []
    }
    mut links = [$"[unreleased]: ($repo)/compare/v($versions.0)...HEAD"]
    mut i = 0
    while $i < ($versions | length) {
        let v = $versions | get $i
        let older = $versions | skip ($i + 1)
        if ($older | is-empty) {
            $links = $links | append $"[($v)]: ($repo)/releases/tag/v($v)"
        } else {
            let prev = $older | first
            $links = $links | append $"[($v)]: ($repo)/compare/v($prev)...v($v)"
        }
        $i = $i + 1
    }
    $links
}

def changelog-notes [text: string, version: string]: nothing -> string {
    let lines = $text | lines
    let heading = $"## [($version)]"
    mut start = -1
    mut i = 0
    for line in $lines {
        if ($line | str starts-with $heading) {
            $start = $i + 1
            break
        }
        $i = $i + 1
    }
    if $start < 0 {
        print --stderr $"error: CHANGELOG.md has no [($version)] section"
        exit 1
    }
    mut end = ($lines | length)
    mut j = $start
    for line in ($lines | skip $start) {
        if ($line | str starts-with "## [") or ($line =~ '^\[.+\]: https?://') {
            $end = $j
            break
        }
        $j = $j + 1
    }
    trim-blank ($lines | skip $start | take ($end - $start)) | str join (char nl)
}

def cut-unreleased [text: string, version: string, repo: string] {
    let date = date now | format date "%Y-%m-%d"
    let lines = $text | lines
    mut u = -1
    mut i = 0
    for line in $lines {
        if $line == "## [Unreleased]" {
            $u = $i
            break
        }
        $i = $i + 1
    }
    if $u < 0 {
        print --stderr "error: CHANGELOG.md has no ## [Unreleased] heading"
        exit 1
    }

    mut next = -1
    mut k = $u + 1
    while $k < ($lines | length) {
        if ($lines | get $k | str starts-with "## [") {
            $next = $k
            break
        }
        $k = $k + 1
    }

    let raw_body = (if $next < 0 {
        $lines | skip ($u + 1)
    } else {
        $lines | skip ($u + 1) | take ($next - $u - 1)
    })
    let body = trim-blank (
        $raw_body | where {|l| not ($l =~ 'in the manifests and has not been published') }
    )
    let items = $body | where {|l| $l =~ '^- '}
    if ($items | is-empty) {
        print --stderr "error: [Unreleased] has no entries to cut"
        exit 1
    }

    let after = drop-link-footer (if $next < 0 { [] } else { $lines | skip $next })
    let headings = ([$version] | append (version-headings $after))
    let preamble = ($lines | take $u)
        | append "## [Unreleased]"
        | append ""
        | append $"## [($version)] - ($date)"
        | append ""
    mut assembled = $preamble | append $body | append "" | append $after
    $assembled = trim-blank $assembled
    let links = make-links $headings $repo
    {
        text: (($assembled | append "" | append $links | str join (char nl)) + (char nl))
        date: $date
    }
}

def bump-toml [text: string, old: string, new: string]: nothing -> string {
    mut in_pkg = false
    mut out = []
    mut replaced_pkg = false
    mut replaced_deps = 0
    for line in ($text | lines) {
        if $line == "[workspace.package]" {
            $in_pkg = true
            $out = $out | append $line
        } else if ($line | str starts-with "[") {
            $in_pkg = false
            $out = $out | append $line
        } else if $in_pkg and $line == $'version = "($old)"' {
            $replaced_pkg = true
            $out = $out | append $'version = "($new)"'
        } else if ($line | str contains 'path = "') and ($line | str contains $'version = "($old)"') {
            $replaced_deps = $replaced_deps + 1
            $out = $out | append ($line | str replace $'version = "($old)"' $'version = "($new)"')
        } else {
            $out = $out | append $line
        }
    }
    if not $replaced_pkg {
        print --stderr $"error: did not find [workspace.package] version = \"($old)\""
        exit 1
    }
    if $replaced_deps == 0 {
        print --stderr $"error: did not find any path-dependency version = \"($old)\""
        exit 1
    }
    ($out | str join (char nl)) + (char nl)
}

def main [
    version?: string  # Version to release. Defaults to the version in Cargo.toml.
    --notes           # Print the GitHub Release body for that version; change nothing.
] {
    cd ($env.FILE_PWD | path dirname)

    let current = workspace-version
    let target = $version | default $current
    parse-semver $target | ignore

    if $notes {
        print (changelog-notes (open --raw CHANGELOG.md) $target)
        return
    }

    if (ver-cmp $target $current) < 0 {
        print --stderr $"error: cannot release ($target) when the workspace is ($current)"
        exit 1
    }

    let text = open --raw CHANGELOG.md
    if (has-section $text $target) {
        print --stderr $"error: CHANGELOG.md already has [($target)]"
        exit 1
    }

    let cut = cut-unreleased $text $target (repo-url)
    $cut.text | save --force CHANGELOG.md

    if $target != $current {
        (bump-toml (open --raw Cargo.toml) $current $target) | save --force Cargo.toml
        # Path-crate versions live in the lockfile too; --locked CI reads them.
        if ("Cargo.lock" | path exists) {
            let lock = (do { ^cargo metadata --format-version 1 --offline } | complete)
            if $lock.exit_code != 0 {
                print --stderr "error: cargo metadata failed; Cargo.lock still names the old version"
                print --stderr $lock.stderr
                exit 1
            }
        }
        print $"==> prepared pdfrum ($target)"
        print $"    Cargo.toml: ($current) -> ($target)"
    } else {
        print $"==> prepared pdfrum ($target)"
        print $"    Cargo.toml: already ($target)"
    }
    print $"    CHANGELOG.md: [Unreleased] -> [($target)] - ($cut.date)"
    print ""
    print "next: commit these files, open a PR, merge to main."
    print $"      the tag-release workflow tags v($target) once CI is green."
}
