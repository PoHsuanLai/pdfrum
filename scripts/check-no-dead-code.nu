#!/usr/bin/env nu
# STYLE.md §4's rule, mechanically: library code carries no dead-code
# suppression.
#
# `#[allow(dead_code)]` in a library is a decision deferred. The lint has
# found an item nothing calls, and there are exactly three honest answers:
#
#   1. Only tests call it — then it belongs under `#[cfg(test)]`, which says
#      so to the compiler instead of asking it to look away.
#   2. Nothing calls it — then it is deleted, and whatever it recorded
#      becomes a sentence in the module doc citing the oracle line.
#   3. It ports oracle behaviour we do not reach any other way — then it is a
#      **missed wire**, not dead code, and it is filed in
#      docs/status/unwired-oracle-ports.md with both citations.
#
# Only the third answer leaves an attribute in the tree, and this check is
# what makes that visible: the reason string must name the registry, so the
# suppression carries its own justification and a reader grepping for the
# registry finds every item that claims it. An attribute with any other
# reason — or none — is one of the first two answers not yet given.
#
# `#[expect(dead_code)]` is the same deferral with a different spelling and is
# treated identically.
#
# Scope: `crates/*/src`, the library ring. `examples/` and `benches/` are
# exempt — a shared example harness where each binary uses a different subset
# is the one place the attribute is honest, and pdfrum-raster-vello has one.
#
# Run standalone, or via scripts/ci.nu which calls it.

# The doc that has to be named. A suppression citing it is a filed missed
# wire; one that does not is undecided.
const REGISTRY = 'docs/status/unwired-oracle-ports.md'

# Every dead-code suppression under `crates/*/src`, as
# `{file, line, text, cited}` — `cited` being whether the attribute names the
# registry.
#
# `rustfmt` breaks a long attribute across lines, so the reason can be several
# lines below the `dead_code` token. The window below is read from the
# attribute's opening `#[` — found by walking back from the token — to its
# closing `)]`, which is what makes the check independent of how the attribute
# happens to be wrapped today.
def suppressions []: nothing -> list<record> {
    glob crates/*/src/**/*.rs | sort | each {|file|
        let rel = ($file | path relative-to (pwd))
        let lines = (open --raw $file | decode utf-8 | lines)
        # The token itself, whether it sits inside `allow(dead_code)` on one
        # line or alone on its own after rustfmt broke the attribute up.
        $lines | enumerate | where {|r| ($r.item | str replace --all ' ' '') =~ 'dead_code' } | each {|hit|
            # Walk back to the `#[`, forward to the `)]`, and read the whole
            # attribute as one string.
            mut start = $hit.index
            while ($start > 0) and (not (($lines | get $start) =~ '#!?\[')) {
                $start = $start - 1
            }
            mut end = $hit.index
            while ($end < (($lines | length) - 1)) and (not (($lines | get $end) =~ '\)\]')) {
                $end = $end + 1
            }
            let text = ($lines | slice $start..$end | str join ' ' | str trim)
            {file: $rel, line: ($start + 1), text: $text, cited: ($text | str contains $REGISTRY)}
        }
    } | flatten
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    print "==> no dead-code suppression in library code"
    let all = (suppressions)
    let undecided = ($all | where {|s| not $s.cited })

    if not ($undecided | is-empty) {
        print --stderr "error: dead-code suppression in library code:"
        for s in $undecided {
            print --stderr $"  ($s.file):($s.line)"
            print --stderr $"    ($s.text)"
        }
        print --stderr ""
        print --stderr "       STYLE.md §4: library code carries no `#[allow(dead_code)]`"
        print --stderr "       and no `#[expect(dead_code)]`. A helper only tests use lives"
        print --stderr "       under `#[cfg(test)]`; an item with no caller is deleted."
        print --stderr ""
        print --stderr $"       The one exception is a ported oracle behaviour with no"
        print --stderr $"       production caller — a *missed wire*. File it in"
        print --stderr $"       ($REGISTRY) and name that file in the"
        print --stderr "       attribute's `reason`, so the suppression carries its own"
        print --stderr "       justification."
        exit 1
    }

    let filed = ($all | where {|s| $s.cited } | length)
    print "ok: no undecided dead-code suppression under crates/*/src"
    print $"    \(($filed) filed as missed wires, each citing ($REGISTRY)\)"

    # The converse, so the check cannot pass by having stopped looking.
    #
    # `check-no-boa.nu`'s reasoning: an assertion that nothing was found is
    # vacuously true the day the search breaks. The scan is proven live by
    # planting an attribute in a temporary file under the scanned tree and
    # requiring that it be seen — and seen as *undecided*, since a scan that
    # found it but classified it as filed would pass the real check too.
    print "==> the scan does still find one"
    let probe = 'crates/pdfrum-object/src/zz_check_no_dead_code_probe.rs'
    "#[allow(dead_code)]\nfn planted() {}\n" | save --force $probe
    let seen = (do --ignore-errors { suppressions } | default []
        | where {|s| $s.file == $probe and not $s.cited })
    rm --force $probe
    if ($seen | is-empty) {
        print --stderr "error: a planted `#[allow(dead_code)]` was not found —"
        print --stderr "       the scan above is measuring nothing."
        exit 1
    }
    print "ok: a planted attribute is found and reported"

    print ""
    print "dead-code check: green."
}
