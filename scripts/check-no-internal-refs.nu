#!/usr/bin/env nu
# No internal phase or document reference survives into rustdoc.
#
# `///` and `//!` become docs.rs. A host who ran `cargo add pdfrum` and opened
# the crate page does not know what `M15`, `WP4`, `§A.11`, `docs/status/M12.md`
# or `SPEC §15.5` are, and cannot follow any of them: the milestone numbers are
# this project's own vocabulary and the documents are not published. A sentence
# whose only explanation is such a pointer explains nothing to the reader it is
# written for.
#
# The rule the tree follows instead (docs/design/rustdoc.md §4, and STYLE.md's
# rustdoc paragraph): for each citing sentence, ask whether it is useful at
# all. Two kinds survive — an invariant a caller can get wrong, which stays in
# rustdoc **rewritten without the pointer**; and a measured fact that changes
# how the *code* must be read, which becomes a short `//` stating the fact
# rather than "see M12 §1.6". Everything else — which pass decided what, which
# corpus document a number came from, which ruling superseded which — is
# deleted, because git history and the documents themselves already hold it.
#
# Implementation `//` comments are OUT of scope, deliberately and permanently.
# They are not published, the next person editing this code *can* follow
# `docs/status/M15.md`, and the design briefs are exactly where that reasoning
# belongs. This check reads the two doc-comment spellings and nothing else.
#
# Run standalone, or via scripts/ci.nu which calls it.

# What an internal reference looks like, in five alternatives:
#
#   \bM[0-9]{1,2}[a-z]?\b     a milestone — M1, M12, M12b
#   \bWP[0-9]+\b              a work package — WP4, WP11
#   §[A-Z]\.[0-9]             an internal annex section — §A.11, §B.5
#   docs/(status|design|upstream)   a path into an unpublished document tree
#   \b(SPEC|PLAN|STYLE|DEPS)(\.md)?\b   a repository document, either spelling
#
# Three notes on the shapes, each of which is a false positive waiting to
# happen and is why the alternatives are anchored the way they are:
#
#   `\bM[0-9]{1,2}` has no false positive in the tree today, and could acquire
#   one — a matrix element `M11`, an OpenType or a colour-space tag. It is kept
#   because a milestone is exactly what a reader cannot look up, and because
#   the failure mode is loud: the check names the line, and a real false
#   positive is the moment to add a per-line opt-out rather than to widen a
#   silence.
#
#   `§[A-Z]\.[0-9]` is deliberately narrow. `ISO 32000-2 §7.6.4.3.3` and
#   `RFC 4013 §2.5` are *numeric* sections of published standards and never
#   match it; an ISO **annex** does — `ISO/IEC 15444-1 §A.4.1` — and that is a
#   citation a reader can follow, so the ISO/RFC exemption below spares it.
#
#   The document names are word-bounded and the `.md` is optional, because the
#   facade carried `(SPEC §15.8)` where a `SPEC\.md` pattern saw nothing. The
#   word boundary is what keeps `SPECIAL`, `PLANE` and `STYLED` out.
const PATTERN = '\bM[0-9]{1,2}[a-z]?\b|\bWP[0-9]+\b|§[A-Z]\.[0-9]|docs/(status|design|upstream)|\b(SPEC|PLAN|STYLE|DEPS)(\.md)?\b'

# A line that cites a published standard before its section marker. Only the
# `§[A-Z].[0-9]` alternative is spared by it — a line reading
# `ISO/IEC 15444-1 §A.4.1` is a citation a reader can follow, while one reading
# `ISO 32000-1 §12.7, and see M14` still has a milestone in it and still fails.
const STANDARD = '(ISO|RFC)[^§]*§[A-Z]\.[0-9]'

# And this file, which necessarily contains what it forbids: the pattern, the
# prose explaining it, and the line its non-vacuity control plants.
const SELF = 'scripts/check-no-internal-refs.nu'

# Every offending `///` / `//!` line under `crates/*/src`, as `{file, line,
# text}`.
#
# `git grep` over the index rather than a filesystem walk, on
# check-no-absolute-paths.nu's reasoning: the question is what a published
# crate would carry, which is what git tracks.
#
# The pathspec is `:(glob)crates/*/src/**` and the `:(glob)` is load-bearing:
# under git's default matching a bare `*` does not cross a `/`, so
# `crates/*/src` matches **no file at all** and the scan reports a clean tree
# by having looked at nothing. The non-vacuity control below is what caught
# that, which is the whole reason it is here.
#
# The grep matches doc-comment lines carrying the pattern in one expression —
# `^\s*///` or `^\s*//!`, then anything, then an alternative — so a `//` on the
# same subject is never reported. `complete` because `git grep` exits 1 on no
# matches, which is the passing case.
def offenders []: nothing -> list<record> {
    let doc_line = ('^[[:space:]]*//[/!].*(' + $PATTERN + ')')
    let r = (^git grep -nE $doc_line -- ':(glob)crates/*/src/**' $":!($SELF)" | complete)
    if $r.exit_code > 1 {
        print --stderr "error: `git grep` failed; this check has not run."
        $r.stderr | lines | each {|l| print --stderr $"  ($l)" } | ignore
        exit 1
    }
    $r.stdout | lines | where {|l| not ($l | is-empty) } | each {|l|
        let parts = ($l | split column ':' file line text --number 3 | first)
        {file: $parts.file, line: $parts.line, text: ($parts.text | str trim)}
    } | where {|o|
        # Spare an ISO or RFC annex citation, but only when the section marker
        # is the *whole* of why the line matched: re-test it with that one
        # alternative removed.
        let others = '\bM[0-9]{1,2}[a-z]?\b|\bWP[0-9]+\b|docs/(status|design|upstream)|\b(SPEC|PLAN|STYLE|DEPS)(\.md)?\b'
        (not ($o.text =~ $STANDARD)) or ($o.text =~ $others)
    }
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    print "==> no internal phase or document references in rustdoc"
    let found = (offenders)
    if not ($found | is-empty) {
        print --stderr "error: internal references in published doc comments:"
        for o in $found {
            print --stderr $"  ($o.file):($o.line)"
            print --stderr $"    ($o.text)"
        }
        print --stderr ""
        print --stderr "       Nobody on docs.rs can follow these. For each one, ask what the"
        print --stderr "       next reader actually needs:"
        print --stderr ""
        print --stderr "         an invariant a caller can get wrong  ->  keep it in `///`,"
        print --stderr "           rewritten to state the invariant without the pointer;"
        print --stderr "         a measured fact that changes how the code reads  ->  a short"
        print --stderr "           `//` on the body stating the fact;"
        print --stderr "         anything else — which pass, which fixture, which ruling  ->"
        print --stderr "           delete it. git and docs/ already hold it."
        print --stderr ""
        print --stderr "       An ISO or RFC annex section (`ISO/IEC 15444-1 §A.4.1`) is"
        print --stderr "       exempt: a reader can follow it. See docs/design/rustdoc.md §4."
        exit 1
    }
    print "ok: none in any `///` or `//!` under crates/*/src"

    # The converse, so the check cannot pass by having stopped looking.
    #
    # check-no-absolute-paths.nu's reasoning: an assertion that nothing was
    # found is vacuously true the day the search breaks. The plant has to be
    # *tracked*, because this scan reads the index rather than the working
    # tree.
    print "==> the scan does still find one"
    let probe = 'crates/pdfrum-common/src/zz-check-no-internal-refs-probe.rs'
    "//! Planted by the non-vacuity control: see PLAN.md §M15 and WP4.\n" | save --force $probe
    ^git add --intent-to-add $probe
    let seen = (do --ignore-errors { offenders } | default []
        | where {|o| $o.file == $probe })
    ^git rm --force --quiet --cached $probe
    rm --force $probe
    if ($seen | is-empty) {
        print --stderr "error: a planted internal reference was not found —"
        print --stderr "       the scan above is measuring nothing."
        exit 1
    }
    print "ok: a planted reference is found and reported"

    # And the ISO exemption, which is the half a widened pattern would break:
    # spare a real annex citation, and still fail a line that carries both.
    print "==> the ISO exemption spares a citation and not a milestone"
    let iso = 'crates/pdfrum-common/src/zz-check-no-internal-refs-iso.rs'
    ("/// A marker segment (ISO/IEC 15444-1 §A.4.1).\n"
        + "/// Ruled in ISO 32000-1 §12.7 and recorded for M14 §B.5.\n") | save --force $iso
    ^git add --intent-to-add $iso
    let iso_seen = (do --ignore-errors { offenders } | default []
        | where {|o| $o.file == $iso })
    ^git rm --force --quiet --cached $iso
    rm --force $iso
    if ($iso_seen | length) != 1 {
        print --stderr $"error: expected exactly one of the two planted lines to fail, got ($iso_seen | length)."
        for o in $iso_seen { print --stderr $"       ($o.text)" }
        exit 1
    }
    if not ($iso_seen | first | get text | str contains 'M14') {
        print --stderr "error: the wrong line failed — the ISO annex citation is not exempt."
        exit 1
    }
    print "ok: the annex citation passes and the milestone beside it does not"

    print ""
    print "internal-reference check: green."
}
