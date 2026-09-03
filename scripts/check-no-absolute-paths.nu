#!/usr/bin/env nu
# No machine-specific absolute path survives into a tracked file.
#
# A path like `/mnt/data2/…` or `/home/someone/…` is a fact about one
# developer's disk. Committed, it is a script that runs nowhere else, a test
# that fails on a path rather than skipping, and a README whose examples a new
# contributor cannot copy. A GitHub CI checkout has neither directory, so
# every one of them is a step that cannot run there.
#
# The rule the tree follows instead (README.md "Building and testing"): every
# path outside this repository is named by an environment variable with a
# default expressed relative to the repository root — `PDFRUM_ORACLE_CHECKOUT`,
# `PDFRUM_ORACLE_BIN`, `PDFRUM_GOLDENS`, `PDFRUM_TOOL`, `PDFRUM_TARGET_ROOT`,
# `PDFRUM_WORKTREE_ROOT` — resolved in one place per language:
# `scripts/env.nu` for nushell, clap's `env =` for the `conformance` CLI, and
# a six-line function per file elsewhere (STYLE.md §4 forbids a `common`
# module to share it in).
#
# EXEMPT: `docs/reviews/`, `docs/status/` and `docs/design/`. Those are
# **records of runs** — a review transcript, a measurement log, a design brief
# quoting the machine a number came from. Rewriting the path a figure was
# measured on would make the record less true, not more portable, and nothing
# in them executes. Every other tracked file is in scope, `scripts/`, tests,
# READMEs and PLAN.md included.
#
# Run standalone, or via scripts/ci.nu which calls it.

# What a machine-specific absolute path looks like. `/home/<user>/` catches any
# contributor's home directory, not only the one this tree grew up in; the
# `/mnt/data2` half is the specific mount that hosted it.
#
# Both alternatives are anchored to a *path start* — the beginning of the line,
# or a character that cannot be part of a path, which is what a quote, a
# backtick, a space or a `=` is here. Without that anchor `/home/…` matches
# inside `/usr/local/home/test.pdf`, which is a PDF filespec literal in
# `pdfrum-doc`'s round-trip test and not a path on anybody's disk — a false
# positive is how a check like this gets switched off.
#
# Deliberately NOT matched: `/usr/bin/time`, `/proc/*/environ`, `/tmp/…` — a
# path every Linux machine has is not machine-specific.
const PATTERN = '(^|[^A-Za-z0-9_./-])(/mnt/data2|/home/[a-z0-9_-]+)/'

# Directories whose contents are records rather than instructions.
const EXEMPT = ['docs/reviews' 'docs/status' 'docs/design']

# And this file, which necessarily contains what it forbids: the pattern it
# matches on, the prose explaining that pattern, and the path its non-vacuity
# control plants. Exempting the checker from its own check is not a hole — a
# path here is a string literal the script reads, never a location it opens,
# and the alternative is a search that cannot describe what it searches for.
const SELF = 'scripts/check-no-absolute-paths.nu'

# Every offending line in the tracked tree, as `{file, line, text}`.
#
# `git grep` over the index rather than a filesystem walk: the question is
# what a CI checkout would receive, which is exactly what git tracks, and it
# costs nothing to ask git instead of re-implementing `.gitignore`.
#
# `complete` because `git grep` exits 1 on *no matches*, which is the passing
# case here and would otherwise abort the script.
def offenders []: nothing -> list<record> {
    let excludes = (($EXEMPT ++ [$SELF]) | each {|d| $":!($d)" })
    let r = (^git grep -nE $PATTERN -- ...$excludes | complete)
    if $r.exit_code > 1 {
        print --stderr "error: `git grep` failed; this check has not run."
        $r.stderr | lines | each {|l| print --stderr $"  ($l)" } | ignore
        exit 1
    }
    $r.stdout | lines | where {|l| not ($l | is-empty) } | each {|l|
        let parts = ($l | split column ':' file line text --number 3 | first)
        {file: $parts.file, line: $parts.line, text: ($parts.text | str trim)}
    }
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    print "==> no machine-specific absolute paths in tracked files"
    let found = (offenders)
    if not ($found | is-empty) {
        print --stderr "error: machine-specific absolute paths in tracked files:"
        for o in $found {
            print --stderr $"  ($o.file):($o.line)"
            print --stderr $"    ($o.text)"
        }
        print --stderr ""
        print --stderr "       These do not exist in a CI checkout. Name the path with the"
        print --stderr "       environment variable that already stands for it, and let it"
        print --stderr "       default to a location relative to the repository root:"
        print --stderr ""
        print --stderr "         PDFRUM_ORACLE_CHECKOUT  <repo>/../pdfium-c++"
        print --stderr "         PDFRUM_ORACLE_BIN       <checkout>/out/Release/pdfium_test"
        print --stderr "         PDFRUM_GOLDENS          <repo>/conformance/goldens"
        print --stderr "         PDFRUM_TOOL             <repo>/target/release/pdfrum-tool"
        print --stderr "         PDFRUM_TARGET_ROOT      <repo>/../cargo-target"
        print --stderr "         PDFRUM_WORKTREE_ROOT    <repo>/../worktrees"
        print --stderr ""
        print --stderr "       scripts/env.nu resolves them for nushell; the conformance CLI"
        print --stderr "       reads them through clap; elsewhere it is six lines per file."
        print --stderr "       See README.md \"Building and testing\"."
        exit 1
    }
    print $"ok: none outside ($EXEMPT | str join ', ') and this script"

    # The converse, so the check cannot pass by having stopped looking.
    #
    # check-no-dead-code.nu's reasoning: an assertion that nothing was found is
    # vacuously true the day the search breaks. The scan is proven live by
    # planting a path in a file the scan must reach — and the plant has to be
    # *tracked*, because this scan reads the index rather than the working
    # tree, which is a way a rewrite of it could silently stop measuring.
    print "==> the scan does still find one"
    let probe = 'zz-check-no-absolute-paths-probe.txt'
    "/mnt/data2/planted/by/the/non-vacuity/control\n" | save --force $probe
    ^git add --intent-to-add $probe
    let seen = (do --ignore-errors { offenders } | default []
        | where {|o| $o.file == $probe })
    ^git rm --force --quiet --cached $probe
    rm --force $probe
    if ($seen | is-empty) {
        print --stderr "error: a planted absolute path was not found —"
        print --stderr "       the scan above is measuring nothing."
        exit 1
    }
    print "ok: a planted path is found and reported"

    print ""
    print "absolute-path check: green."
}
