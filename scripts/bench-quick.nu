#!/usr/bin/env nu
# The dev loop: three representative documents per class, low sample counts.
#
# Usage: scripts/bench-quick.nu [group ...]
#
#   scripts/bench-quick.nu                 # every group
#   scripts/bench-quick.nu render-warm     # just the warm render groups
#   scripts/bench-quick.nu open build      # the two cheap ones
#
# The full suite is `scripts/ci.nu`'s big sibling: 44 documents x eleven groups,
# about an hour, and it is what `benches/baseline.json` and every table in
# the internal working notes are taken from. It is not a thing to run between two edits.
# This is: **eighteen documents, three per class**, at criterion's floor rather
# than the suite's window, which lands in three to four minutes.
#
# What that buys and what it does not. It buys the direction of a change on
# every class — enough to tell an optimization from a pessimization before
# committing to a real measurement. It does **not** buy a number anyone should
# write down: at these sample counts the confidence interval is several times
# the ratchet's noise band, and the ratchet is deliberately not run here. A
# result from this script is a reason to run the full suite, never a substitute
# for having run it.
#
# The eighteen are chosen for *spread inside* the class rather than for weight:
# one document at each end of the class's cost range and one in the middle, so a
# change that helps large inputs and hurts small ones shows up as two rows
# moving in opposite directions instead of one average that moved a little. See
# benches/corpus/PROVENANCE.md for what each file is.

# Three per class: cheap, middling, heavy. The names are criterion filters —
# it matches on a substring of the benchmark id, and every id is
# `<group>/<class>/<stem>`, so a bare stem selects that document in every group.
const DOCUMENTS = [
    text_bug_1029 text_cjk_page text_tcpdf_055
    vector_tcpdf_009 vector_en_tem vector_paths_1751
    image_ccitt_transfer image_jpx_123 image_jbig2_880920
    shading_type4_5 shading_tcpdf_058 shading_axial_radial
    forms_number forms_text_field forms_widgets_407
    mixed_formfield mixed_tcpdf_059 mixed_en_uicase
]

# Which crate owns which group, since M12's per-crate split. A group named on
# the command line runs only its own crate's bench binary; with none named,
# all five run.
const OWNER = {
    open: pdfrum-parser
    build: pdfrum-page
    render-cold: pdfrum-render
    render-warm: pdfrum-render
    text: pdfrum-text
    save: pdfrum-edit
}

# Criterion's own floor. `--sample-size 10` is the minimum it accepts and
# `--warm-up-time 1` still lets a warm group's cache fill; below either, the
# numbers stop being repeatable even to the one significant figure this script
# is for. `--measurement-time 1` is what makes it three minutes instead of forty.
const CRITERION_ARGS = [--sample-size 10 --warm-up-time 1 --measurement-time 1 --noplot]

def main [...groups: string] {
    cd ($env.FILE_PWD | path dirname)

    let unknown = ($groups | where {|g| $g not-in ($OWNER | columns) })
    if not ($unknown | is-empty) {
        print --stderr $"error: unknown group '($unknown | first)'"
        print --stderr $"known: ($OWNER | columns | str join ' ')"
        exit 2
    }

    # De-duplicate the owners: `render-cold render-warm` is one crate, not two
    # runs of it, and the group name goes into the filter instead.
    let crates = if ($groups | is-empty) {
        [pdfrum-parser pdfrum-page pdfrum-render pdfrum-text pdfrum-edit]
    } else {
        $groups | each {|g| $OWNER | get $g } | uniq
    }

    # Criterion takes one regex. When a group was named, the id has to match
    # both the group and one of the eighteen documents, which a single regex
    # expresses only as a product — so build it that way rather than passing
    # two filters criterion would AND incorrectly.
    let docs = ($DOCUMENTS | str join '|')
    let pattern = if ($groups | is-empty) {
        $"/\(($docs)\)$"
    } else {
        $"^\(($groups | str join '|')\)[^/]*/[^/]*/\(($docs)\)$"
    }

    print $"==> bench-quick: 18 documents, ($crates | length) crate\(s\), criterion floor"
    print $"    pattern: ($pattern)"
    print "    NOT a ratchet run — see the header of this script before quoting a number."
    print ""

    for crate in $crates {
        print $"==> ($crate)"
        ^cargo bench -p $crate -- ...$CRITERION_ARGS $pattern
        print ""
    }

    print "bench-quick done. For a number that counts:"
    print "  cargo bench --workspace"
    print "  cargo run --release -p pdfrum-bench --bin ratchet -- check"
}
